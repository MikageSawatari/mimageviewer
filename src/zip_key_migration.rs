//! CP932 ZIP エントリ名デコード対応 (v1.4.0) に伴う、リリース済み per-page キーの
//! 一度きり移行。
//!
//! v1.3.x まで、非 UTF-8 名 ZIP のエントリ名は zip crate の既定 (CP437) デコードの
//! mojibake 文字列で、★レーティング・補正・マスク・隠蔽・補正レイヤー・テキスト注釈・
//! 出力範囲の各 DB キー (`adjustment_db::zip_entry_key` 形式 `<zip>::<entry>`) と
//! 代表サムネピン (`folder_thumb_pins.source_entry`) はその mojibake 名から導出されて
//! いた。v1.4.0 の Shift-JIS デコードで entry_name が正しくなった結果、旧キーが
//! 参照されなくなる (= ユーザーデータが見えなくなる) ため、列挙時に得た
//! `(旧名, 新名)` ペア ([`crate::zip_loader::ZipEnumeration::legacy_renames`]) で
//! 旧キー → 新キーへ書き換える。
//!
//! - **冪等**: `UPDATE OR IGNORE` + 旧行 `DELETE`。新キーに既に行があれば新を優先し
//!   旧行は捨てる (v1.4.0 で先に付け直したデータを mojibake 時代の値で潰さない)。
//! - **worker スレッドで実行**: 呼び出しは [`spawn_if_needed`] 経由。UI スレッドで
//!   DB を触らない。セッション内は ZIP パスごとに 1 回だけ走る。
//! - **対象外 (許容済みの制限)**: ネスト ZIP ツリーの「本」単位キー
//!   (rating の ZipDir 合成キー / spread / 入れ子ピン container_key)。これらは
//!   ディレクトリ prefix 由来の合成キーで再構築が複雑なわりに、消えても 1 操作で
//!   付け直せる軽量データのため、移行対象から外した (ページ単位の実データを優先)。

use std::collections::HashSet;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

/// `(<db ファイル名>, <テーブル>, <キー列>)`。
/// 列挙はスキーマの正本 (各モジュールの CREATE TABLE) と一致させること。
const PAGE_KEY_TARGETS: &[(&str, &str, &str)] = &[
    ("rating.db", "ratings", "path"),
    ("adjustment.db", "page_params", "page_path"),
    ("mask.db", "masks", "path"),
    ("conceal.db", "conceal_entries", "page_path"),
    ("local_adjust.db", "local_adjust_pages", "page_path"),
    ("comic.db", "comic_entries", "page_path"),
    ("export_crop.db", "export_crop_pages", "page_path"),
];

/// セッション内で移行済みの ZIP (normalize 済みパス)。
static MIGRATED_THIS_SESSION: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// 列挙で CP932 リネームが検出された ZIP の per-page キー移行を同期実行する。
///
/// **worker スレッドから呼ぶこと** (UI スレッド禁止 — SQLite を複数開く)。
/// 列挙直後・結果を UI へ送る前に呼べば、開いた直後の ★/補正の読み出しが
/// 必ず移行後のキーに当たる (race なし)。`renames` が空 (= UTF-8 名 ZIP の通常経路)
/// は即 return。セッション内は ZIP パスごとに 1 回だけ走る。
pub fn migrate_if_needed(zip_path: &Path, renames: &[(String, String)]) {
    if renames.is_empty() {
        return;
    }
    let session_key = crate::adjustment_db::normalize_path(zip_path);
    {
        let Ok(mut done) = MIGRATED_THIS_SESSION.lock() else {
            return;
        };
        if !done.insert(session_key) {
            return;
        }
    }
    let migrated = run_migration_at(&crate::data_dir::get(), zip_path, renames);
    if migrated > 0 {
        crate::logger::log(format!(
            "[ZIPKEY] CP932 legacy key migration: {} ({} renames, {} rows)",
            zip_path.display(),
            renames.len(),
            migrated
        ));
    }
}

/// 実際の移行。書き換えた行数の合計を返す (テスト可能なように data_dir を引数化)。
pub fn run_migration_at(data_dir: &Path, zip_path: &Path, renames: &[(String, String)]) -> usize {
    let mut total = 0usize;
    for (file, table, col) in PAGE_KEY_TARGETS {
        let db_path = data_dir.join(file);
        if !db_path.exists() {
            continue;
        }
        match migrate_page_keys(&db_path, table, col, zip_path, renames) {
            Ok(n) => total += n,
            Err(e) => crate::logger::log(format!(
                "[ZIPKEY] migration failed: {file} {table}.{col}: {e}"
            )),
        }
    }
    match migrate_thumb_pin(&data_dir.join("folder_thumb_pins.db"), zip_path, renames) {
        Ok(n) => total += n,
        Err(e) => crate::logger::log(format!("[ZIPKEY] pin migration failed: {e}")),
    }
    total
}

fn migrate_page_keys(
    db_path: &Path,
    table: &str,
    col: &str,
    zip_path: &Path,
    renames: &[(String, String)],
) -> Result<usize, rusqlite::Error> {
    let mut conn = rusqlite::Connection::open(db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let tx = conn.transaction()?;
    let mut changed = 0usize;
    {
        // UPDATE OR IGNORE: 新キー側に既に行があれば旧行はそのまま残り、直後の
        // DELETE で破棄される (新データ優先)。
        let mut update = tx.prepare(&format!(
            "UPDATE OR IGNORE {table} SET {col} = ?1 WHERE {col} = ?2"
        ))?;
        let mut delete = tx.prepare(&format!("DELETE FROM {table} WHERE {col} = ?1"))?;
        for (old_entry, new_entry) in renames {
            let old_key = crate::adjustment_db::zip_entry_key(zip_path, old_entry);
            let new_key = crate::adjustment_db::zip_entry_key(zip_path, new_entry);
            if old_key == new_key {
                continue;
            }
            changed += update.execute(rusqlite::params![new_key, old_key])?;
            delete.execute([&old_key])?;
        }
    }
    tx.commit()?;
    Ok(changed)
}

/// 代表サムネピン: `container_key` (ZIP 自身) + `source_entry` (エントリ名そのまま)。
fn migrate_thumb_pin(
    db_path: &Path,
    zip_path: &Path,
    renames: &[(String, String)],
) -> Result<usize, rusqlite::Error> {
    if !db_path.exists() {
        return Ok(0);
    }
    let mut conn = rusqlite::Connection::open(db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let container_key = crate::path_key::normalize_keep_drive(zip_path);
    let tx = conn.transaction()?;
    let mut changed = 0usize;
    {
        let mut update = tx.prepare(
            "UPDATE folder_thumb_pins SET source_entry = ?1
             WHERE container_key = ?2 AND source_entry = ?3",
        )?;
        for (old_entry, new_entry) in renames {
            changed += update.execute(rusqlite::params![new_entry, container_key, old_entry])?;
        }
    }
    tx.commit()?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 旧キーの行が新キーへ移り、新キーに既存行がある場合は新が優先される。
    #[test]
    fn migrates_page_keys_and_prefers_existing_new_rows() {
        let dir = tempfile::tempdir().unwrap();
        let zip = PathBuf::from(r"D:\Comics\本.zip");
        let old_entry = "ﾓｼﾞﾊﾞｹ/p1.jpg"; // CP437 mojibake 相当 (中身は何でもよい)
        let new_entry = "新しい名前/p1.jpg";
        let old_entry2 = "ﾓｼﾞﾊﾞｹ/p2.jpg";
        let new_entry2 = "新しい名前/p2.jpg";

        let db_path = dir.path().join("rating.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL)",
            )
            .unwrap();
            let old_key = crate::adjustment_db::zip_entry_key(&zip, old_entry);
            let old_key2 = crate::adjustment_db::zip_entry_key(&zip, old_entry2);
            let new_key2 = crate::adjustment_db::zip_entry_key(&zip, new_entry2);
            conn.execute(
                "INSERT INTO ratings (path, stars) VALUES (?1, 4)",
                [&old_key],
            )
            .unwrap();
            // p2 は旧キーと新キーの両方に行がある (v1.4.0 で付け直し済みの想定)
            conn.execute(
                "INSERT INTO ratings (path, stars) VALUES (?1, 2)",
                [&old_key2],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ratings (path, stars) VALUES (?1, 5)",
                [&new_key2],
            )
            .unwrap();
        }

        let renames = vec![
            (old_entry.to_string(), new_entry.to_string()),
            (old_entry2.to_string(), new_entry2.to_string()),
        ];
        let migrated = run_migration_at(dir.path(), &zip, &renames);
        assert!(migrated >= 1);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let stars: i64 = conn
            .query_row(
                "SELECT stars FROM ratings WHERE path = ?1",
                [&crate::adjustment_db::zip_entry_key(&zip, new_entry)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stars, 4, "旧キーの★が新キーへ移る");
        let stars2: i64 = conn
            .query_row(
                "SELECT stars FROM ratings WHERE path = ?1",
                [&crate::adjustment_db::zip_entry_key(&zip, new_entry2)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stars2, 5, "新キーに既存行があれば新を優先 (旧 2 を捨てる)");
        let old_left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ratings WHERE path LIKE '%ﾓｼﾞﾊﾞｹ%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_left, 0, "旧キー行は残らない (冪等)");

        // 再実行しても安全 (冪等)。
        let again = run_migration_at(dir.path(), &zip, &renames);
        assert_eq!(again, 0);
    }

    /// 代表サムネピンの source_entry も新名へ移行される。
    #[test]
    fn migrates_thumb_pin_source_entry() {
        let dir = tempfile::tempdir().unwrap();
        let zip = PathBuf::from(r"D:\Comics\本.zip");
        let db_path = dir.path().join("folder_thumb_pins.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS folder_thumb_pins (
                    container_key TEXT PRIMARY KEY,
                    source_kind   TEXT NOT NULL,
                    source_rel    TEXT NOT NULL,
                    source_entry  TEXT,
                    source_page   INTEGER
                )",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO folder_thumb_pins
                 (container_key, source_kind, source_rel, source_entry)
                 VALUES (?1, 'zip', '', ?2)",
                rusqlite::params![crate::path_key::normalize_keep_drive(&zip), "ﾓｼﾞﾊﾞｹ/p1.jpg"],
            )
            .unwrap();
        }

        let renames = vec![("ﾓｼﾞﾊﾞｹ/p1.jpg".to_string(), "新しい名前/p1.jpg".to_string())];
        let migrated = run_migration_at(dir.path(), &zip, &renames);
        assert_eq!(migrated, 1);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let entry: String = conn
            .query_row(
                "SELECT source_entry FROM folder_thumb_pins WHERE container_key = ?1",
                [&crate::path_key::normalize_keep_drive(&zip)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entry, "新しい名前/p1.jpg");
    }
}
