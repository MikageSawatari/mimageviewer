//! お気に入り配下のフォルダ・ZIP・PDF・動画名を記録する検索インデックス DB。
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
    VideoFile = 3,
}

impl IndexKind {
    fn from_i64(v: i64) -> Option<Self> {
        match v {
            0 => Some(Self::Folder),
            1 => Some(Self::ZipFile),
            2 => Some(Self::PdfFile),
            3 => Some(Self::VideoFile),
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

/// `delete_subtree` / `prune_stale_under_subtree` で共用する境界判定。
/// 戻り値 `(root_norm, prefix)`:
/// - `root_norm` は `root_path` を `normalize_path` した結果 (root 行一致比較用)。
/// - `prefix` は常に末尾 `/` で終わる文字列 (root_norm にすでに `/` があれば 1 つだけ、
///   無ければ追加)。SQL 側で `?2 || '/'` を組むと `c:/` 等の drive root で `//` に
///   なる事故を避けるため、Rust 側で済ませる。
fn subtree_bounds(root_path: &Path) -> (String, String) {
    let root_norm = normalize_path(root_path);
    let prefix = if root_norm.ends_with('/') {
        root_norm.clone()
    } else {
        format!("{}/", root_norm)
    };
    (root_norm, prefix)
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

    /// `root_path` 自身と配下のすべての行を `favorite_root` スコープで削除する。
    /// notify-rs の差分追従 (`apply_single_change`) で「フォルダごと消えた」「サブツリーごと
    /// 消えた」を検知したときに呼ぶ。`upsert_children` の DELETE は親直下しか触れないので、
    /// 観測できない孫以下を一括で掃除するためにこのメソッドが必要。
    ///
    /// 境界判定は wildcard (`LIKE`) を使わない: `path` に `%` / `_` が含まれる ZIP/PDF 名で
    /// 過剰削除しないため、Rust 側で trailing slash 付きの `prefix` を組んで `substr` で
    /// 比較する (Codex P1 レビュー指摘)。
    ///
    /// 戻り値: 削除行数 (診断用)。
    pub fn delete_subtree(
        &self,
        favorite_root: &Path,
        root_path: &Path,
    ) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        let fav_norm = normalize_path(favorite_root);
        let (root_norm, prefix) = subtree_bounds(root_path);
        conn.execute(
            "DELETE FROM entries \
             WHERE favorite_root = ?1 \
             AND ( path = ?2 OR substr(path, 1, length(?3)) = ?3 )",
            params![fav_norm, root_norm, prefix],
        )
    }

    /// `root_path` 配下 (root 自身を含む) のうち、`updated_at < cutoff` の行だけを削除する。
    /// `apply_single_change` の subtree recursive upsert 後に呼ぶ post-scan prune 用。
    ///
    /// 同名ディレクトリの「fast delete → recreate (子孫が減る)」シナリオでは、新しい実体に
    /// 以前の深い子孫が無いので recursive upsert が訪問しない → 古い孫行が `updated_at`
    /// 古いまま残留する。`run_bulk_name_index` の `prune_stale_for_favorite` と同じ
    /// stamp 方式を subtree scope で適用することで掃除する。
    ///
    /// 境界判定は `delete_subtree` と同じ wildcard なしの substr 比較 (helper
    /// `subtree_bounds` で共有)。
    ///
    /// 呼び出し側 (`apply_single_change`) は **subtree scan が clean に完了したときのみ**
    /// 呼ぶこと。途中 cancel / `read_dir` error 等で不完全な観測になった状態で呼ぶと、
    /// 正当な行を stale と誤判定して消してしまう。
    pub fn prune_stale_under_subtree(
        &self,
        favorite_root: &Path,
        root_path: &Path,
        updated_at_cutoff: i64,
    ) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        let fav_norm = normalize_path(favorite_root);
        let (root_norm, prefix) = subtree_bounds(root_path);
        conn.execute(
            "DELETE FROM entries \
             WHERE favorite_root = ?1 \
             AND ( path = ?2 OR substr(path, 1, length(?3)) = ?3 ) \
             AND updated_at < ?4",
            params![fav_norm, root_norm, prefix, updated_at_cutoff],
        )
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
    /// `favorite_roots` が空の場合は (種別フィルタを除き) 全 favorite 対象。
    /// `mode` で include トークン結合を AND/OR 切替。
    /// `kind` が `Some(k)` のときは種別で AND 絞り込みする。`None` ("すべて") でも
    /// 動画 (VideoFile) は常に除外し、Folder/ZIP/PDF だけ返す
    /// (Ctrl+S 種別フィルタ、docs/search-container-item-redesign.md §4.2 / §task#4)。
    ///
    /// クエリ構文は `search_query::parse` を参照。トークンごとに
    /// `name LIKE ?` / `name NOT LIKE ?` を生成し、include は `mode` で結合、
    /// NOT は常に AND で追加する (docs §20)。`%` `_` `\` は ESCAPE 節でリテラル扱い。
    pub fn search(
        &self,
        query: &str,
        favorite_roots: &[PathBuf],
        kind: Option<IndexKind>,
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

        // 種別フィルタ (Ctrl+S §4.2)。動画はコンテナ索引の対象外 (§task#4) なので、
        // Some(k) なら指定種別だけ、None ("すべて") でも VideoFile を除外して
        // Folder/ZIP/PDF だけ返す。旧バージョンが書いた VideoFile 行が DB に
        // 残っていてもクエリ側で弾く (kind 値は enum 由来の定数なのでリテラル埋め込み可)。
        let kind_val: Option<i64> = kind.map(|k| k as i64);
        match kind_val {
            Some(_) => where_clauses.push("kind = ?".to_string()),
            None => where_clauses.push(format!("kind <> {}", IndexKind::VideoFile as i64)),
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

        // バインドを WHERE 節と同じ順序で積む: include (まとめて) → exclude (1 個ずつ)
        // → お気に入り → 種別。
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
        if let Some(ref kv) = kind_val {
            params_vec.push(kv as &dyn rusqlite::ToSql);
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
        let mut stmt =
            conn.prepare("SELECT favorite_root, COUNT(*) FROM entries GROUP BY favorite_root")?;
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
        crate::logger::log("search_index_db: migrating PRIMARY KEY (path) → (favorite_root, path)");
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
        match WRITE_STAMP.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
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
        match WRITE_STAMP.compare_exchange_weak(prev, floor, Ordering::Relaxed, Ordering::Relaxed) {
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
                entry(r"C:\FavA\four.mp4", "four.mp4", IndexKind::VideoFile),
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
        assert_eq!(counts.get(&key_a), Some(&4));
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
            entry(r"C:\Fav\sub\delta.mp4", "delta.mp4", IndexKind::VideoFile),
        ];
        db.upsert_children(&fav, &parent, &children).unwrap();
        assert_eq!(db.total_count().unwrap(), 4);

        let results = db
            .search("alp", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display_name, "alpha");

        let results = db
            .search(".zip", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, IndexKind::ZipFile);

        // 動画はコンテナ索引の対象外: 旧 DB 由来の VideoFile 行があっても
        // kind=None ("すべて") の検索結果には出てこない (§task#4)。
        let results = db
            .search(".mp4", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert!(results.is_empty(), "動画はコンテナ検索結果から除外される");

        // 大文字小文字無視
        let results = db
            .search("BETA", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
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
        let all = db
            .search("", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
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
        let results = db
            .search("", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(results[0].display_name, "b");
    }

    #[test]
    fn search_filters_by_kind() {
        // Ctrl+S 種別フィルタ (§4.2): kind=Some(k) で kind 列を AND 絞り込みする。
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        db.upsert_children(
            &fav,
            &fav,
            &[
                entry(r"C:\Fav\photos", "photos", IndexKind::Folder),
                entry(r"C:\Fav\photos.zip", "photos.zip", IndexKind::ZipFile),
                entry(r"C:\Fav\photos.pdf", "photos.pdf", IndexKind::PdfFile),
                // 旧バージョン由来の VideoFile 行を混ぜておく (§task#4 のクエリ側除外を検証)。
                entry(r"C:\Fav\photos.mp4", "photos.mp4", IndexKind::VideoFile),
            ],
        )
        .unwrap();
        // kind=None → 動画を除いた 3 種別のみ (VideoFile 行はクエリで弾かれる)
        let all = db
            .search("photos", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(
            all.len(),
            3,
            "kind=None でも動画は除外され Folder/ZIP/PDF の 3 種別"
        );
        assert!(
            all.iter().all(|e| e.kind != IndexKind::VideoFile),
            "結果に VideoFile が混ざらない"
        );
        // kind=Some(ZipFile) → ZIP のみ
        let zips = db
            .search(
                "photos",
                &[],
                Some(IndexKind::ZipFile),
                crate::search_query::MatchMode::And,
            )
            .unwrap();
        assert_eq!(zips.len(), 1);
        assert_eq!(zips[0].kind, IndexKind::ZipFile);
        // kind=Some(Folder) → フォルダのみ
        let folders = db
            .search(
                "photos",
                &[],
                Some(IndexKind::Folder),
                crate::search_query::MatchMode::And,
            )
            .unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].kind, IndexKind::Folder);
    }

    #[test]
    fn search_kind_filter_combined_with_favorites_excludes_and_or() {
        // バインド順序の回帰検出: include(OR) → exclude → favorite_roots → kind を
        // すべて同時に使い、SQL のプレースホルダと params_vec が一致していることを確認する。
        let db = open_mem();
        let fav1 = PathBuf::from(r"C:\Fav1");
        let fav2 = PathBuf::from(r"C:\Fav2");
        let fav3 = PathBuf::from(r"C:\Fav3");
        db.upsert_children(
            &fav1,
            &fav1,
            &[
                entry(r"C:\Fav1\photos.zip", "photos.zip", IndexKind::ZipFile),
                entry(r"C:\Fav1\photos.pdf", "photos.pdf", IndexKind::PdfFile),
                entry(
                    r"C:\Fav1\docs-draft.zip",
                    "docs-draft.zip",
                    IndexKind::ZipFile,
                ),
            ],
        )
        .unwrap();
        db.upsert_children(
            &fav2,
            &fav2,
            &[entry(r"C:\Fav2\docs.zip", "docs.zip", IndexKind::ZipFile)],
        )
        .unwrap();
        db.upsert_children(
            &fav3,
            &fav3,
            &[entry(
                r"C:\Fav3\photos.zip",
                "photos.zip",
                IndexKind::ZipFile,
            )],
        )
        .unwrap();

        // include=photos OR docs, exclude=draft, roots=[fav1,fav2], kind=ZipFile
        let results = db
            .search(
                "photos docs -draft",
                &[fav1.clone(), fav2.clone()],
                Some(IndexKind::ZipFile),
                crate::search_query::MatchMode::Or,
            )
            .unwrap();
        let names: Vec<&str> = results.iter().map(|e| e.display_name.as_str()).collect();
        assert_eq!(
            results.len(),
            2,
            "photos.zip(fav1) と docs.zip(fav2) のみ: {names:?}"
        );
        assert!(names.contains(&"photos.zip"), "{names:?}");
        assert!(names.contains(&"docs.zip"), "{names:?}");
        // docs-draft.zip は exclude、photos.pdf は kind 不一致、fav3 は roots 外。
        assert!(!names.contains(&"docs-draft.zip"));
        assert!(!names.contains(&"photos.pdf"));
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
            .search(
                "match",
                &[fav1.clone()],
                None,
                crate::search_query::MatchMode::And,
            )
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
            .search("alpha beta", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].display_name, "alpha_beta");

        // 片方しかないと落ちる
        let r = db
            .search(
                "alpha epsilon",
                &[],
                None,
                crate::search_query::MatchMode::And,
            )
            .unwrap();
        assert_eq!(r.len(), 0);

        // OR モードなら片方だけでも拾える
        let r = db
            .search(
                "alpha epsilon",
                &[],
                None,
                crate::search_query::MatchMode::Or,
            )
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
            .search("image -bad", &[], None, crate::search_query::MatchMode::And)
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
            .search(
                "klee nsfw -sleep",
                &[],
                None,
                crate::search_query::MatchMode::Or,
            )
            .unwrap();
        let names: Vec<&str> = r.iter().map(|e| e.display_name.as_str()).collect();
        assert!(names.contains(&"klee"));
        assert!(names.contains(&"nsfw_art"));
        assert!(!names.contains(&"klee_sleep"), "sleep を含むのは常に除外");
        assert!(
            !names.contains(&"other"),
            "include にマッチしない doc は除外"
        );
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
            .search(
                r#""hello world""#,
                &[],
                None,
                crate::search_query::MatchMode::And,
            )
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
            .search("100_", &[], None, crate::search_query::MatchMode::And)
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

    // -----------------------------------------------------------------------
    // delete_subtree / prune_stale_under_subtree
    // (B4: watcher 経路の subtree prune 用 — Codex レビュー反映)
    // -----------------------------------------------------------------------

    #[test]
    fn subtree_bounds_basic() {
        let (root, prefix) = subtree_bounds(Path::new(r"C:\Foo\Bar"));
        assert_eq!(root, "c:/foo/bar");
        assert_eq!(prefix, "c:/foo/bar/");
    }

    #[test]
    fn subtree_bounds_drive_root_does_not_double_slash() {
        // `c:\` → normalize_path で "c:/" になるので、prefix は二重 '/' にならないこと
        let (root, prefix) = subtree_bounds(Path::new(r"C:\"));
        assert_eq!(root, "c:/");
        assert_eq!(prefix, "c:/");
    }

    #[test]
    fn delete_subtree_removes_root_and_descendants() {
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        // /Fav/sub と /Fav/sub/deep/x.zip を入れる
        db.upsert_children(
            &fav,
            &fav,
            &[entry(r"C:\Fav\sub", "sub", IndexKind::Folder)],
        )
        .unwrap();
        db.upsert_children(
            &fav,
            &PathBuf::from(r"C:\Fav\sub"),
            &[entry(r"C:\Fav\sub\deep", "deep", IndexKind::Folder)],
        )
        .unwrap();
        db.upsert_children(
            &fav,
            &PathBuf::from(r"C:\Fav\sub\deep"),
            &[entry(r"C:\Fav\sub\deep\x.zip", "x.zip", IndexKind::ZipFile)],
        )
        .unwrap();
        assert_eq!(db.total_count().unwrap(), 3);

        let n = db
            .delete_subtree(&fav, &PathBuf::from(r"C:\Fav\sub"))
            .unwrap();
        // sub, deep, x.zip の 3 行が消える
        assert_eq!(n, 3);
        assert_eq!(db.total_count().unwrap(), 0);
    }

    #[test]
    fn delete_subtree_respects_path_boundary_foo_vs_foobar() {
        // `c:/fav/foo` を消そうとして `c:/fav/foobar` まで消えないこと
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        db.upsert_children(
            &fav,
            &fav,
            &[
                entry(r"C:\Fav\foo", "foo", IndexKind::Folder),
                entry(r"C:\Fav\foobar", "foobar", IndexKind::Folder),
            ],
        )
        .unwrap();
        // foo の下に x.zip
        db.upsert_children(
            &fav,
            &PathBuf::from(r"C:\Fav\foo"),
            &[entry(r"C:\Fav\foo\x.zip", "x.zip", IndexKind::ZipFile)],
        )
        .unwrap();
        // foobar の下に y.zip (これは残るべき)
        db.upsert_children(
            &fav,
            &PathBuf::from(r"C:\Fav\foobar"),
            &[entry(r"C:\Fav\foobar\y.zip", "y.zip", IndexKind::ZipFile)],
        )
        .unwrap();
        assert_eq!(db.total_count().unwrap(), 4);

        let n = db
            .delete_subtree(&fav, &PathBuf::from(r"C:\Fav\foo"))
            .unwrap();
        // foo と foo/x.zip の 2 行のみ消える
        assert_eq!(n, 2);

        let r = db
            .search("y.zip", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(r.len(), 1, "foobar 配下の y.zip は残るべき");
        let r = db
            .search("foobar", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(r.len(), 1, "foobar Folder 行も残るべき");
    }

    #[test]
    fn delete_subtree_treats_like_wildcards_as_literal() {
        // path に `%` `_` が含まれていても LIKE 評価されず、誤削除されない
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        db.upsert_children(
            &fav,
            &fav,
            &[
                entry(r"C:\Fav\foo_%", "foo_%", IndexKind::Folder),
                entry(r"C:\Fav\foo%bar", "foo%bar", IndexKind::Folder),
                entry(r"C:\Fav\fooXY", "fooXY", IndexKind::Folder),
            ],
        )
        .unwrap();
        assert_eq!(db.total_count().unwrap(), 3);

        // `foo_%` を消す (LIKE で評価されると `foo` + 任意 1 文字 + 任意文字列 で
        // マッチして fooXY や foo%bar まで消える危険があるシナリオ)
        let n = db
            .delete_subtree(&fav, &PathBuf::from(r"C:\Fav\foo_%"))
            .unwrap();
        assert_eq!(n, 1, "literal な foo_% のみ消える (LIKE 評価されない)");

        let r = db
            .search("fooXY", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(r.len(), 1, "fooXY は残るべき (LIKE 評価で巻き込まれない)");
        let r = db
            .search("foo%bar", &[], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(r.len(), 1, "foo%bar は残るべき (LIKE 評価で巻き込まれない)");
    }

    #[test]
    fn delete_subtree_scoped_to_favorite_root() {
        // nested favorites: 親 fav の delete_subtree が子 fav の同一 path 行を巻き込まない
        let db = open_mem();
        let fav_parent = PathBuf::from(r"C:\Fav");
        let fav_child = PathBuf::from(r"C:\Fav\sub");
        // 両 fav にそれぞれ同じ実体 path を登録
        db.upsert_children(
            &fav_parent,
            &fav_parent,
            &[entry(r"C:\Fav\sub", "sub", IndexKind::Folder)],
        )
        .unwrap();
        db.upsert_children(
            &fav_parent,
            &PathBuf::from(r"C:\Fav\sub"),
            &[entry(r"C:\Fav\sub\x.zip", "x.zip", IndexKind::ZipFile)],
        )
        .unwrap();
        db.upsert_children(
            &fav_child,
            &fav_child,
            &[entry(r"C:\Fav\sub\x.zip", "x.zip", IndexKind::ZipFile)],
        )
        .unwrap();
        assert_eq!(db.total_count().unwrap(), 3);

        // 親 fav スコープで sub の subtree を delete
        db.delete_subtree(&fav_parent, &PathBuf::from(r"C:\Fav\sub"))
            .unwrap();

        // 親 fav 配下は 0 件
        let r = db
            .search(
                "x.zip",
                &[fav_parent.clone()],
                None,
                crate::search_query::MatchMode::And,
            )
            .unwrap();
        assert!(r.is_empty(), "親 fav 配下は消えるべき");

        // 子 fav スコープは無傷
        let r = db
            .search(
                "x.zip",
                &[fav_child.clone()],
                None,
                crate::search_query::MatchMode::And,
            )
            .unwrap();
        assert_eq!(r.len(), 1, "子 fav 配下は巻き込まれない");
    }

    #[test]
    fn prune_stale_under_subtree_removes_root_self_if_older_than_cutoff() {
        // `prune_stale_under_subtree` は `path = root_path` 自身の行も cutoff 対象になる。
        // この不変量は「scan_start を parent refresh の前に取る」設計の意図を固定する
        // (Codex 第 9 レビュー指摘)。
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        // /Fav/sub を put (これが「root_path 自身の行」)
        db.upsert_children(
            &fav,
            &fav,
            &[entry(r"C:\Fav\sub", "sub", IndexKind::Folder)],
        )
        .unwrap();
        assert_eq!(db.total_count().unwrap(), 1);

        // 上記 upsert の updated_at より大きい cutoff を取れば、stale 扱いで消える
        let cutoff = next_write_stamp();
        let n = db
            .prune_stale_under_subtree(&fav, &PathBuf::from(r"C:\Fav\sub"), cutoff)
            .unwrap();
        assert_eq!(n, 1, "root_path 自身の行が cutoff 対象になるべき");
        assert_eq!(db.total_count().unwrap(), 0);
    }

    #[test]
    fn prune_stale_under_subtree_keeps_fresh_descendants() {
        // subtree scope 内で `updated_at >= cutoff` の行は残る
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        // 古い行を入れる
        db.upsert_children(
            &fav,
            &PathBuf::from(r"C:\Fav\sub"),
            &[entry(r"C:\Fav\sub\old.zip", "old.zip", IndexKind::ZipFile)],
        )
        .unwrap();

        // cutoff を取る
        let cutoff = next_write_stamp();

        // 新しい行を入れる (cutoff より大きい stamp)
        db.upsert_children(
            &fav,
            &PathBuf::from(r"C:\Fav\sub"),
            &[
                entry(r"C:\Fav\sub\old.zip", "old.zip", IndexKind::ZipFile),
                entry(r"C:\Fav\sub\new.zip", "new.zip", IndexKind::ZipFile),
            ],
        )
        .unwrap();
        // upsert_children は同じ parent の既存行を DELETE → INSERT し直すので、
        // old.zip の updated_at も新しくなっている
        assert_eq!(db.total_count().unwrap(), 2);

        // cutoff より古い行は残っていないので prune は 0 件 (= scope 内全部 fresh)
        let n = db
            .prune_stale_under_subtree(&fav, &PathBuf::from(r"C:\Fav\sub"), cutoff)
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(db.total_count().unwrap(), 2);
    }

    #[test]
    fn prune_stale_under_subtree_does_not_touch_outside_scope() {
        // subtree 外 / 他 favorite は cutoff 比較されない
        let db = open_mem();
        let fav_a = PathBuf::from(r"C:\FavA");
        let fav_b = PathBuf::from(r"C:\FavB");
        db.upsert_children(
            &fav_a,
            &fav_a,
            &[
                entry(
                    r"C:\FavA\target_subtree",
                    "target_subtree",
                    IndexKind::Folder,
                ),
                entry(r"C:\FavA\other_sibling", "other_sibling", IndexKind::Folder),
            ],
        )
        .unwrap();
        db.upsert_children(
            &fav_b,
            &fav_b,
            &[entry(r"C:\FavB\x", "x", IndexKind::Folder)],
        )
        .unwrap();
        assert_eq!(db.total_count().unwrap(), 3);

        // 全行が cutoff より古い状態で prune を呼ぶ
        let cutoff = next_write_stamp();
        db.prune_stale_under_subtree(&fav_a, &PathBuf::from(r"C:\FavA\target_subtree"), cutoff)
            .unwrap();

        // target_subtree のみが消え、other_sibling と FavB は残る
        assert!(
            db.search(
                "target_subtree",
                &[],
                None,
                crate::search_query::MatchMode::And
            )
            .unwrap()
            .is_empty(),
            "target_subtree は消えるべき"
        );
        assert_eq!(
            db.search(
                "other_sibling",
                &[],
                None,
                crate::search_query::MatchMode::And
            )
            .unwrap()
            .len(),
            1,
            "subtree 外の sibling は残る"
        );
        assert_eq!(
            db.search("x", &[], None, crate::search_query::MatchMode::And)
                .unwrap()
                .len(),
            1,
            "他 favorite は巻き込まれない"
        );
    }

    #[test]
    fn prune_stale_under_subtree_drive_root_safety() {
        // root_path が drive root (`C:\`) のとき、prefix が二重 '/' にならず正しくマッチ
        let db = open_mem();
        let fav = PathBuf::from(r"C:\");
        db.upsert_children(&fav, &fav, &[entry(r"C:\foo", "foo", IndexKind::Folder)])
            .unwrap();
        // 古い stamp の行を残したまま、新しい cutoff で prune
        let cutoff = next_write_stamp();
        let n = db.prune_stale_under_subtree(&fav, &fav, cutoff).unwrap();
        assert_eq!(n, 1, "drive root でも prefix が正しくマッチする");
    }
}
