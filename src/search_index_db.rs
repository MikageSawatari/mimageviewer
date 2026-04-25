//! お気に入り配下のフォルダ・ZIP・PDF 名を記録する検索インデックス DB。
//!
//! `%APPDATA%/mimageviewer/search_index.db` に単一の SQLite ファイルとして保存される。
//! - ブラウズ時の差分 upsert (お気に入り配下に入ったフォルダの直下アイテム)
//! - 「お気に入り > インデックス作成」での一括再構築
//! - 「お気に入り > 検索」での部分一致検索
//!
//! パスは `normalize_path` で 小文字化 + バックスラッシュ→スラッシュ に正規化して
//! PRIMARY KEY にする (rotation_db / adjustment_db / catalog と同じ規約)。
//! ドライブ文字は保持する (お気に入りフォルダごとのスコープ判定に必要)。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use rusqlite::{Connection, params};

// -----------------------------------------------------------------------
// 種別
// -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    Folder = 0,
    ZipFile = 1,
    PdfFile = 2,
}

impl IndexKind {
    fn from_i64(v: i64) -> Option<Self> {
        match v {
            0 => Some(Self::Folder),
            1 => Some(Self::ZipFile),
            2 => Some(Self::PdfFile),
            _ => None,
        }
    }
}

// -----------------------------------------------------------------------
// エントリ
// -----------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// 元のファイル・フォルダ完全パス (表示・ナビゲーション用)
    pub path: PathBuf,
    /// 表示用ファイル名 (ディレクトリの場合はフォルダ名)
    pub display_name: String,
    pub kind: IndexKind,
    pub mtime: i64,
}

// -----------------------------------------------------------------------
// パス正規化
// -----------------------------------------------------------------------

/// パスを小文字化 + バックスラッシュ→スラッシュに正規化する。
/// お気に入りのスコープ判定に使うため、ドライブ文字 (C:) は保持する。
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
}

// -----------------------------------------------------------------------
// SearchIndexDb
// -----------------------------------------------------------------------

pub struct SearchIndexDb {
    conn: Mutex<Connection>,
}

impl SearchIndexDb {
    /// `%APPDATA%/mimageviewer/search_index.db` を開く (なければ作成)。
    pub fn open() -> rusqlite::Result<Self> {
        let db_path = crate::data_dir::get().join("search_index.db");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// メモリ上に空の DB を開く (テスト / 仮計算用)。
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 任意パスに開く (統合テスト用)。`data_dir` に依存せずディスク永続の DB を作れる。
    pub fn open_at(db_path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 親フォルダ配下の指定アイテムを一括 upsert する。
    /// 親フォルダ直下の既存エントリのうち、`children` に含まれないものは削除する
    /// (差分反映)。
    ///
    /// **Codex P2 #2 対応 (2026-04)**: DELETE / INSERT 両方とも `favorite_root` で
    /// スコープを絞る。複合 PK `(favorite_root, path)` へ移行したため、nested favorites
    /// (親 / 子が同じ実体を指す) で互いに行を上書きしない設計に揃える。
    /// (tests/search_name_e2e.rs::nested_favorites_both_scopes_find_shared_path)
    pub fn upsert_children(
        &self,
        favorite_root: &Path,
        parent: &Path,
        children: &[IndexEntry],
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        // 親フォルダ直下の既存エントリを一度消してから入れ直す
        // (LIKE 'parent_norm/%' で直下以外も消えるのを避けるため、パス区切り単位で比較)
        let parent_norm = normalize_path(parent);
        // 直下 = path が "parent_norm/{name}" で '/' が parent_norm の直後に 1 回のみ
        let prefix = if parent_norm.ends_with('/') {
            parent_norm.clone()
        } else {
            format!("{}/", parent_norm)
        };
        let fav_norm = normalize_path(favorite_root);
        // 直下判定: path LIKE 'prefix%' かつ substr(path, len(prefix)+1) に '/' を含まない
        // かつ **favorite_root が一致する** 行のみ対象 (nested favorites で他 fav の
        // 行を巻き込まないように)。
        tx.execute(
            "DELETE FROM entries \
             WHERE favorite_root = ?1 \
             AND path LIKE ?2 || '%' \
             AND instr(substr(path, length(?2) + 1), '/') = 0",
            params![fav_norm, prefix],
        )?;

        let now = next_write_stamp();
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO entries \
                 (path, display_path, name, display_name, kind, favorite_root, \
                  mtime, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for entry in children {
                let path_norm = normalize_path(&entry.path);
                let name_lower = entry.display_name.to_lowercase();
                let display_path = entry.path.to_string_lossy().to_string();
                stmt.execute(params![
                    path_norm,
                    display_path,
                    name_lower,
                    entry.display_name,
                    entry.kind as i64,
                    fav_norm,
                    entry.mtime,
                    now,
                ])?;
            }
        }
        tx.commit()
    }

    /// `favorite_root` 配下で `updated_at < cutoff` の行を一括削除する。
    ///
    /// フルバルクスキャン完了後に呼ぶ。`upsert_children` は親フォルダ直下の行しか
    /// DELETE しないため、アプリ停止中に親フォルダごと消えたサブツリーの孫行は
    /// upsert の経路で掃除できない。cutoff = scan 開始時に取った
    /// `next_write_stamp()` にすれば、scan 中の upsert は **strictly greater** な
    /// stamp を取るので `updated_at >= cutoff`、未観測の stale 行は `< cutoff` で
    /// 分離できる。`next_write_stamp` は process-wide atomic で単調増加なので、
    /// 同秒で連続スキャンしても cutoff が衝突しない (Codex P2 回帰対策)。
    ///
    /// `favorite_root = ?` スコープなので nested favorites の他 favorite の行は
    /// 巻き込まない。戻り値: 削除行数 (診断用)。
    pub fn prune_stale_for_favorite(
        &self,
        favorite_root: &Path,
        updated_at_cutoff: i64,
    ) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        let fav_norm = normalize_path(favorite_root);
        let affected = conn.execute(
            "DELETE FROM entries \
             WHERE favorite_root = ?1 \
             AND updated_at < ?2",
            params![fav_norm, updated_at_cutoff],
        )?;
        Ok(affected)
    }

    /// インデックス作成時に、お気に入り配下のエントリを全削除する。
    pub fn clear_for_favorite(&self, favorite_root: &Path) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        let fav_norm = normalize_path(favorite_root);
        conn.execute(
            "DELETE FROM entries WHERE favorite_root = ?1",
            params![fav_norm],
        )?;
        Ok(())
    }

    /// お気に入りに含まれない (不要になった) エントリを削除する。
    /// `active_roots` は現行のお気に入りの正規化済みパス集合。
    pub fn prune_obsolete(&self, active_roots: &[String]) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        if active_roots.is_empty() {
            // お気に入りがない場合は全削除
            conn.execute("DELETE FROM entries", [])?;
            return Ok(());
        }
        // IN 句を動的に組み立てる (お気に入りは最大 20 件なので文字列連結で十分)
        let placeholders: Vec<String> = (1..=active_roots.len())
            .map(|i| format!("?{}", i))
            .collect();
        let sql = format!(
            "DELETE FROM entries WHERE favorite_root NOT IN ({})",
            placeholders.join(",")
        );
        let params_vec: Vec<&dyn rusqlite::ToSql> = active_roots
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();
        conn.execute(&sql, params_vec.as_slice())?;
        Ok(())
    }

    /// 部分一致検索 (大文字小文字無視)。結果は表示名で昇順ソート済み。
    /// `favorite_roots` が空の場合は全件対象。`mode` で include トークン結合を AND/OR 切替。
    ///
    /// クエリ構文は `search_query::parse` を参照。トークンごとに
    /// `name LIKE ?` / `name NOT LIKE ?` を生成し、include は `mode` で結合、
    /// NOT は常に AND で追加する (docs §20)。`%` `_` `\` は ESCAPE 節でリテラル扱い。
    pub fn search(
        &self,
        query: &str,
        favorite_roots: &[PathBuf],
        mode: crate::search_query::MatchMode,
    ) -> rusqlite::Result<Vec<IndexEntry>> {
        let tokens = crate::search_query::parse(query);

        let conn = self.conn.lock().unwrap();

        // include と exclude を分離する: include は `mode` に従って結合、exclude は常に AND。
        let mut include_clauses: Vec<&str> = Vec::new();
        let mut exclude_clauses: Vec<&str> = Vec::new();
        let mut include_binds: Vec<String> = Vec::new();
        let mut exclude_binds: Vec<String> = Vec::new();
        for t in &tokens {
            if t.include {
                include_binds.push(format!("%{}%", escape_like(&t.needle)));
                include_clauses.push("name LIKE ? ESCAPE '\\'");
            } else {
                exclude_binds.push(format!("%{}%", escape_like(&t.needle)));
                exclude_clauses.push("name NOT LIKE ? ESCAPE '\\'");
            }
        }

        let mut where_clauses: Vec<String> = Vec::new();
        if !include_clauses.is_empty() {
            let joiner = match mode {
                crate::search_query::MatchMode::And => " AND ",
                crate::search_query::MatchMode::Or => " OR ",
            };
            where_clauses.push(format!("({})", include_clauses.join(joiner)));
        }
        for c in &exclude_clauses {
            where_clauses.push((*c).to_string());
        }

        let fav_norm_strs: Vec<String> = favorite_roots.iter().map(|p| normalize_path(p)).collect();
        if !fav_norm_strs.is_empty() {
            let placeholders = vec!["?"; fav_norm_strs.len()].join(",");
            where_clauses.push(format!("favorite_root IN ({placeholders})"));
        }

        let where_sql = if where_clauses.is_empty() {
            "1=1".to_string()
        } else {
            where_clauses.join(" AND ")
        };

        let sql = format!(
            "SELECT display_path, display_name, kind, mtime \
             FROM entries \
             WHERE {where_sql} \
             ORDER BY display_name COLLATE NOCASE \
             LIMIT 5000"
        );

        let mut stmt = conn.prepare(&sql)?;

        // バインドを WHERE 節と同じ順序で積む: include (まとめて) → exclude (1 個ずつ) → お気に入り。
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::new();
        for s in &include_binds {
            params_vec.push(s as &dyn rusqlite::ToSql);
        }
        for s in &exclude_binds {
            params_vec.push(s as &dyn rusqlite::ToSql);
        }
        for s in &fav_norm_strs {
            params_vec.push(s as &dyn rusqlite::ToSql);
        }

        let rows = stmt.query_map(params_vec.as_slice(), |row| {
            let display_path: String = row.get(0)?;
            let display_name: String = row.get(1)?;
            let kind_i: i64 = row.get(2)?;
            let mtime: i64 = row.get(3)?;
            Ok(IndexEntry {
                path: PathBuf::from(display_path),
                display_name,
                kind: IndexKind::from_i64(kind_i).unwrap_or(IndexKind::Folder),
                mtime,
            })
        })?;

        Ok(rows.flatten().collect())
    }

    /// DB 内の総エントリ数を返す (UI 表示用)。
    pub fn total_count(&self) -> rusqlite::Result<u64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM entries", [], |r| {
            let v: i64 = r.get(0)?;
            Ok(v as u64)
        })
    }

    /// 全 favorite のエントリ数を 1 クエリで返す (UI 表示の一括集計用)。
    ///
    /// 個別 `count_for_favorite` を N 回呼ぶと毎回 connection mutex を取り
    /// background writer (bulk indexer / supervisor) と競合するため、まとめて取得する。
    /// 返却 key は `normalize_path(favorite_root)` 後の文字列 (= DB に格納されている形)。
    pub fn count_grouped_by_favorite_root(
        &self,
    ) -> rusqlite::Result<std::collections::HashMap<String, u64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT favorite_root, COUNT(*) FROM entries GROUP BY favorite_root")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for r in rows {
            let (k, c) = r?;
            out.insert(k, c as u64);
        }
        Ok(out)
    }

    /// 指定お気に入り配下のエントリ数を返す (UI 表示用)。
    pub fn count_for_favorite(&self, favorite_root: &Path) -> rusqlite::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let fav_norm = normalize_path(favorite_root);
        conn.query_row(
            "SELECT COUNT(*) FROM entries WHERE favorite_root = ?1",
            params![fav_norm],
            |r| {
                let v: i64 = r.get(0)?;
                Ok(v as u64)
            },
        )
    }

    /// 指定お気に入り配下にエントリが 1 件でもあるか。
    /// `count_for_favorite` と異なり **`EXISTS` なので O(log N)** で早く返る。
    /// バルク起動判定など「0 件か否か」だけを知りたい場面で使う。
    pub fn has_any_for_favorite(&self, favorite_root: &Path) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let fav_norm = normalize_path(favorite_root);
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM entries WHERE favorite_root = ?1 LIMIT 1)",
            params![fav_norm],
            |r| r.get::<_, bool>(0),
        )
    }
}

// -----------------------------------------------------------------------
// スキーマ
// -----------------------------------------------------------------------

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    // 新規 DB 用: 複合 PRIMARY KEY `(favorite_root, path)` で作る。
    // 同じ実体 path が複数 favorite に所属する (nested favorites) ケースを表現できる。
    // idx_entries_fav_updated は `prune_stale_for_favorite` の
    // `WHERE favorite_root = ? AND updated_at < ?` を index-only で解決するため
    // の複合 index。favorite_root を先頭に置いているので
    // `WHERE favorite_root = ?` 単独クエリ (search / count_for_favorite /
    // has_any_for_favorite / clear_for_favorite) も prefix でこの index を使える。
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entries (
             path                  TEXT NOT NULL,
             display_path          TEXT NOT NULL,
             name                  TEXT NOT NULL,
             display_name          TEXT NOT NULL,
             kind                  INTEGER NOT NULL,
             favorite_root         TEXT NOT NULL,
             mtime                 INTEGER NOT NULL DEFAULT 0,
             updated_at            INTEGER NOT NULL DEFAULT 0,
             PRIMARY KEY (favorite_root, path)
         );
         CREATE INDEX IF NOT EXISTS idx_entries_name ON entries(name);
         CREATE INDEX IF NOT EXISTS idx_entries_fav_updated \
             ON entries(favorite_root, updated_at);
         -- 旧 idx_entries_fav は idx_entries_fav_updated が prefix で置換できるので落とす
         DROP INDEX IF EXISTS idx_entries_fav;",
    )?;

    // Migration 1 (2026-04): コミット 7883750 で `favorite_root_display` カラムを
    // 削除したが、旧バージョンで作られた既存 DB には `NOT NULL` 制約付きで残存して
    // いる。新しい INSERT 文には載っていないため、upsert_children で
    // `NOT NULL constraint failed: entries.favorite_root_display` が発生する。
    // SQLite 3.35+ の `ALTER TABLE DROP COLUMN` で落とす (rusqlite bundled は新しい)。
    let has_obsolete_col: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('entries') WHERE name = 'favorite_root_display'",
        [],
        |r| r.get(0),
    )?;
    if has_obsolete_col > 0 {
        conn.execute("ALTER TABLE entries DROP COLUMN favorite_root_display", [])?;
        crate::logger::log(
            "search_index_db: migrated — dropped obsolete favorite_root_display column",
        );
    }

    // Migration 2 (2026-04 Codex P2 #2): PRIMARY KEY を `path` 単独から
    // `(favorite_root, path)` 複合に移行する。単独 PK では nested favorites で
    // 同じ path を別 favorite が上書きしてしまい、片方の検索スコープから欠落する
    // バグがあった (tests/search_name_e2e.rs::nested_favorites_both_scopes_find_shared_path)。
    //
    // SQLite は PRIMARY KEY を直接変更できないため、空の場合は recreate、データがある
    // 場合は entries_new を作って ingest → swap する。
    let old_pk_is_path_only: bool = {
        let mut stmt = conn.prepare("PRAGMA table_info('entries')")?;
        let mut rows = stmt.query([])?;
        let mut pk_cols: Vec<(i64, String)> = Vec::new();
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            let pk: i64 = row.get(5)?;
            if pk > 0 {
                pk_cols.push((pk, name));
            }
        }
        pk_cols.len() == 1 && pk_cols[0].1 == "path"
    };
    if old_pk_is_path_only {
        crate::logger::log(
            "search_index_db: migrating PRIMARY KEY (path) → (favorite_root, path)",
        );
        conn.execute_batch(
            "CREATE TABLE entries_new (
                 path                  TEXT NOT NULL,
                 display_path          TEXT NOT NULL,
                 name                  TEXT NOT NULL,
                 display_name          TEXT NOT NULL,
                 kind                  INTEGER NOT NULL,
                 favorite_root         TEXT NOT NULL,
                 mtime                 INTEGER NOT NULL DEFAULT 0,
                 updated_at            INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (favorite_root, path)
             );
             INSERT OR IGNORE INTO entries_new \
                 (path, display_path, name, display_name, kind, favorite_root, mtime, updated_at) \
                 SELECT path, display_path, name, display_name, kind, favorite_root, mtime, updated_at \
                 FROM entries;
             DROP TABLE entries;
             ALTER TABLE entries_new RENAME TO entries;
             CREATE INDEX IF NOT EXISTS idx_entries_name ON entries(name);
             CREATE INDEX IF NOT EXISTS idx_entries_fav_updated \
                 ON entries(favorite_root, updated_at);",
        )?;
        crate::logger::log("search_index_db: migration complete");
    }

    // 前回プロセスが書き込んだ最大 stamp で WRITE_STAMP floor を引き上げる。
    // 壁時計が後退していても `next_write_stamp` が逆行しないことを保証する。
    let max_stamp: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(updated_at), 0) FROM entries",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if max_stamp > 0 {
        bump_write_stamp_floor(max_stamp);
    }

    Ok(())
}

// -----------------------------------------------------------------------
// 単調増加 write stamp (scan cutoff と updated_at の両方に使う)
// -----------------------------------------------------------------------

/// `entries.updated_at` と `prune_stale_for_favorite` の cutoff に書き込むための
/// プロセス全域で **厳密に単調増加** な i64 スタンプ。
///
/// 秒精度 timestamp だと、連続スキャン (scan A の upsert → すぐ scan B の start)
/// が同じ秒に入った瞬間 stale 行の `updated_at == cutoff` となり prune されず
/// 残留するバグがあった (Codex P2 指摘)。AtomicI64 + `max(prev+1, now_ns)` で
/// 毎呼び出し一意な値を返す設計に切り替えて、同秒の連続スキャンでも衝突しない。
///
/// 意味的には「ナノ秒エポック」に近いが、単調性を担保するため atomic floor で
/// `max(prev+1, now_ns)` を返す (壁時計が後退しても counter は戻らない)。
/// 値のスケールそのものは外から観測しないので `SystemTime::now` の単位には依存しない。
static WRITE_STAMP: AtomicI64 = AtomicI64::new(0);

/// 新しい write stamp を発行する。
/// - 戻り値は、このプロセスでの過去の `next_write_stamp` 呼び出しすべてより厳密に大きい。
/// - シード値は `SystemTime::now().as_nanos()`。再起動後も壁時計の単調性が保たれる限り
///   前回プロセスが書き込んだ最大値より大きくなる (DB open 時に floor を bump するので
///   壁時計の後退にも耐える)。
pub(crate) fn next_write_stamp() -> i64 {
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    let mut prev = WRITE_STAMP.load(Ordering::Relaxed);
    loop {
        let next = std::cmp::max(prev.saturating_add(1), now_ns);
        match WRITE_STAMP.compare_exchange_weak(
            prev,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return next,
            Err(v) => prev = v,
        }
    }
}

/// WRITE_STAMP の floor を `floor` 以上に引き上げる。
/// DB open 時に `MAX(updated_at)` を渡して、前回プロセスの書き込み後に
/// 壁時計が後退していても単調性が保たれるようにする。
fn bump_write_stamp_floor(floor: i64) {
    let mut prev = WRITE_STAMP.load(Ordering::Relaxed);
    while prev < floor {
        match WRITE_STAMP.compare_exchange_weak(
            prev,
            floor,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(v) => prev = v,
        }
    }
}

/// SQL LIKE のワイルドカード (`%` `_`) と ESCAPE 文字 (`\`) をエスケープする。
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\\' || c == '%' || c == '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// -----------------------------------------------------------------------
// お気に入りスコープ判定ヘルパー
// -----------------------------------------------------------------------

/// `path` が `favorite_root` 配下 (または一致) かを、正規化済み文字列で判定する。
pub fn is_under(path: &Path, favorite_root: &Path) -> bool {
    let path_norm = normalize_path(path);
    let fav_norm = normalize_path(favorite_root);
    if path_norm == fav_norm {
        return true;
    }
    // prefix + '/' で境界マッチ (途中一致で誤ヒットしないように)
    let fav_with_sep = if fav_norm.ends_with('/') {
        fav_norm
    } else {
        format!("{}/", fav_norm)
    };
    path_norm.starts_with(&fav_with_sep)
}

// -----------------------------------------------------------------------
// テスト
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn open_mem() -> SearchIndexDb {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        SearchIndexDb {
            conn: Mutex::new(conn),
        }
    }

    fn entry(path: &str, name: &str, kind: IndexKind) -> IndexEntry {
        IndexEntry {
            path: PathBuf::from(path),
            display_name: name.to_string(),
            kind,
            mtime: 0,
        }
    }

    #[test]
    fn normalize_path_basic() {
        assert_eq!(normalize_path(Path::new(r"C:\Foo\Bar")), "c:/foo/bar");
    }

    #[test]
    fn is_under_positive() {
        assert!(is_under(
            Path::new(r"C:\Photos\2024\summer"),
            Path::new(r"C:\Photos"),
        ));
        assert!(is_under(Path::new(r"C:\Photos"), Path::new(r"C:\Photos")));
    }

    #[test]
    fn is_under_negative() {
        assert!(!is_under(Path::new(r"C:\Photos2"), Path::new(r"C:\Photos"),));
        assert!(!is_under(Path::new(r"D:\Photos"), Path::new(r"C:\Photos")));
    }

    #[test]
    fn count_grouped_by_favorite_root_returns_per_fav_counts() {
        let db = open_mem();
        let fav_a = PathBuf::from(r"C:\FavA");
        let fav_b = PathBuf::from(r"C:\FavB");
        db.upsert_children(
            &fav_a,
            &PathBuf::from(r"C:\FavA"),
            &[
                entry(r"C:\FavA\one", "one", IndexKind::Folder),
                entry(r"C:\FavA\two.zip", "two.zip", IndexKind::ZipFile),
                entry(r"C:\FavA\three.pdf", "three.pdf", IndexKind::PdfFile),
            ],
        )
        .unwrap();
        db.upsert_children(
            &fav_b,
            &PathBuf::from(r"C:\FavB"),
            &[entry(r"C:\FavB\only", "only", IndexKind::Folder)],
        )
        .unwrap();

        let counts = db.count_grouped_by_favorite_root().unwrap();
        let key_a = normalize_path(&fav_a);
        let key_b = normalize_path(&fav_b);
        assert_eq!(counts.get(&key_a), Some(&3));
        assert_eq!(counts.get(&key_b), Some(&1));
    }

    #[test]
    fn upsert_and_search() {
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        let parent = PathBuf::from(r"C:\Fav\sub");
        let children = vec![
            entry(r"C:\Fav\sub\alpha", "alpha", IndexKind::Folder),
            entry(r"C:\Fav\sub\beta.zip", "beta.zip", IndexKind::ZipFile),
            entry(r"C:\Fav\sub\gamma.pdf", "gamma.pdf", IndexKind::PdfFile),
        ];
        db.upsert_children(&fav, &parent, &children).unwrap();
        assert_eq!(db.total_count().unwrap(), 3);

        let results = db.search("alp", &[], crate::search_query::MatchMode::And).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display_name, "alpha");

        let results = db.search(".zip", &[], crate::search_query::MatchMode::And).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, IndexKind::ZipFile);

        // 大文字小文字無視
        let results = db.search("BETA", &[], crate::search_query::MatchMode::And).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn upsert_replaces_siblings_only() {
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        let parent_a = PathBuf::from(r"C:\Fav\A");
        let parent_b = PathBuf::from(r"C:\Fav\B");

        db.upsert_children(
            &fav,
            &parent_a,
            &[
                entry(r"C:\Fav\A\x", "x", IndexKind::Folder),
                entry(r"C:\Fav\A\y", "y", IndexKind::Folder),
            ],
        )
        .unwrap();
        db.upsert_children(
            &fav,
            &parent_b,
            &[entry(r"C:\Fav\B\z", "z", IndexKind::Folder)],
        )
        .unwrap();
        assert_eq!(db.total_count().unwrap(), 3);

        // A 配下を再 upsert (y を消して w を追加)、B は触らない
        db.upsert_children(
            &fav,
            &parent_a,
            &[
                entry(r"C:\Fav\A\x", "x", IndexKind::Folder),
                entry(r"C:\Fav\A\w", "w", IndexKind::Folder),
            ],
        )
        .unwrap();
        let all = db.search("", &[], crate::search_query::MatchMode::And).unwrap();
        assert_eq!(all.len(), 3);
        let names: Vec<&str> = all.iter().map(|e| e.display_name.as_str()).collect();
        assert!(names.contains(&"x"));
        assert!(names.contains(&"w"));
        assert!(names.contains(&"z"));
        assert!(!names.contains(&"y"));
    }

    #[test]
    fn clear_for_favorite() {
        let db = open_mem();
        let fav1 = PathBuf::from(r"C:\Fav1");
        let fav2 = PathBuf::from(r"C:\Fav2");
        db.upsert_children(&fav1, &fav1, &[entry(r"C:\Fav1\a", "a", IndexKind::Folder)])
            .unwrap();
        db.upsert_children(&fav2, &fav2, &[entry(r"C:\Fav2\b", "b", IndexKind::Folder)])
            .unwrap();
        assert_eq!(db.total_count().unwrap(), 2);

        db.clear_for_favorite(&fav1).unwrap();
        assert_eq!(db.total_count().unwrap(), 1);
        let results = db.search("", &[], crate::search_query::MatchMode::And).unwrap();
        assert_eq!(results[0].display_name, "b");
    }

    #[test]
    fn search_filtered_by_favorite() {
        let db = open_mem();
        let fav1 = PathBuf::from(r"C:\Fav1");
        let fav2 = PathBuf::from(r"C:\Fav2");
        db.upsert_children(
            &fav1,
            &fav1,
            &[entry(r"C:\Fav1\match", "match", IndexKind::Folder)],
        )
        .unwrap();
        db.upsert_children(
            &fav2,
            &fav2,
            &[entry(r"C:\Fav2\match", "match", IndexKind::Folder)],
        )
        .unwrap();
        let results = db
            .search("match", &[fav1.clone()], crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, PathBuf::from(r"C:\Fav1\match"));
    }

    #[test]
    fn search_and_tokens() {
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        db.upsert_children(
            &fav,
            &fav,
            &[
                entry(r"C:\Fav\alpha_beta", "alpha_beta", IndexKind::Folder),
                entry(r"C:\Fav\alpha_gamma", "alpha_gamma", IndexKind::Folder),
                entry(r"C:\Fav\delta", "delta", IndexKind::Folder),
            ],
        )
        .unwrap();

        // AND: 両方含むもの
        let r = db
            .search("alpha beta", &[], crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].display_name, "alpha_beta");

        // 片方しかないと落ちる
        let r = db
            .search("alpha epsilon", &[], crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(r.len(), 0);

        // OR モードなら片方だけでも拾える
        let r = db
            .search("alpha epsilon", &[], crate::search_query::MatchMode::Or)
            .unwrap();
        let names: Vec<&str> = r.iter().map(|e| e.display_name.as_str()).collect();
        assert!(names.contains(&"alpha_beta"));
        assert!(names.contains(&"alpha_gamma"));
        assert!(!names.contains(&"delta"));
    }

    #[test]
    fn search_not_tokens() {
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        db.upsert_children(
            &fav,
            &fav,
            &[
                entry(r"C:\Fav\good_image", "good_image", IndexKind::Folder),
                entry(r"C:\Fav\bad_image", "bad_image", IndexKind::Folder),
                entry(r"C:\Fav\other", "other", IndexKind::Folder),
            ],
        )
        .unwrap();

        let r = db
            .search("image -bad", &[], crate::search_query::MatchMode::And)
            .unwrap();
        let names: Vec<&str> = r.iter().map(|e| e.display_name.as_str()).collect();
        assert!(names.contains(&"good_image"));
        assert!(!names.contains(&"bad_image"));
        assert!(!names.contains(&"other"));
    }

    #[test]
    fn search_or_mode_with_excludes() {
        // docs §20: OR でも NOT は AND 扱い
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        db.upsert_children(
            &fav,
            &fav,
            &[
                entry(r"C:\Fav\klee", "klee", IndexKind::Folder),
                entry(r"C:\Fav\klee_sleep", "klee_sleep", IndexKind::Folder),
                entry(r"C:\Fav\nsfw_art", "nsfw_art", IndexKind::Folder),
                entry(r"C:\Fav\other", "other", IndexKind::Folder),
            ],
        )
        .unwrap();

        // "klee nsfw -sleep" OR → (klee OR nsfw) AND (NOT sleep)
        let r = db
            .search("klee nsfw -sleep", &[], crate::search_query::MatchMode::Or)
            .unwrap();
        let names: Vec<&str> = r.iter().map(|e| e.display_name.as_str()).collect();
        assert!(names.contains(&"klee"));
        assert!(names.contains(&"nsfw_art"));
        assert!(!names.contains(&"klee_sleep"), "sleep を含むのは常に除外");
        assert!(!names.contains(&"other"), "include にマッチしない doc は除外");
    }

    #[test]
    fn search_phrase() {
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        db.upsert_children(
            &fav,
            &fav,
            &[
                entry(r"C:\Fav\hello world", "hello world", IndexKind::Folder),
                entry(r"C:\Fav\hello there", "hello there", IndexKind::Folder),
            ],
        )
        .unwrap();

        let r = db
            .search(r#""hello world""#, &[], crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].display_name, "hello world");
    }

    #[test]
    fn search_like_wildcards_are_literal() {
        // '_' や '%' を含むエントリ名は SQL LIKE でそのまま照合される
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        db.upsert_children(
            &fav,
            &fav,
            &[
                entry(r"C:\Fav\100_percent", "100_percent", IndexKind::Folder),
                entry(r"C:\Fav\abcpercent", "abcpercent", IndexKind::Folder),
            ],
        )
        .unwrap();

        // `_` はワイルドカードではなくリテラルの underscore として扱う
        let r = db
            .search("100_", &[], crate::search_query::MatchMode::And)
            .unwrap();
        let names: Vec<&str> = r.iter().map(|e| e.display_name.as_str()).collect();
        assert_eq!(names, vec!["100_percent"]);
    }

    #[test]
    fn prune_obsolete() {
        let db = open_mem();
        let fav_keep = PathBuf::from(r"C:\Keep");
        let fav_drop = PathBuf::from(r"C:\Drop");
        db.upsert_children(
            &fav_keep,
            &fav_keep,
            &[entry(r"C:\Keep\a", "a", IndexKind::Folder)],
        )
        .unwrap();
        db.upsert_children(
            &fav_drop,
            &fav_drop,
            &[entry(r"C:\Drop\b", "b", IndexKind::Folder)],
        )
        .unwrap();
        assert_eq!(db.total_count().unwrap(), 2);
        db.prune_obsolete(&[normalize_path(&fav_keep)]).unwrap();
        assert_eq!(db.total_count().unwrap(), 1);
    }
}
