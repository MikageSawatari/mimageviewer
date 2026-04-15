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

use rusqlite::{params, Connection};

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

    /// 親フォルダ配下の指定アイテムを一括 upsert する。
    /// 親フォルダ直下の既存エントリのうち、`children` に含まれないものは削除する
    /// (差分反映)。
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
        // 直下判定: path LIKE 'prefix%' かつ substr(path, len(prefix)+1) に '/' を含まない
        tx.execute(
            "DELETE FROM entries \
             WHERE path LIKE ?1 || '%' \
             AND instr(substr(path, length(?1) + 1), '/') = 0",
            params![prefix],
        )?;

        let fav_norm = normalize_path(favorite_root);
        let now = chrono_now_secs();
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
        let params_vec: Vec<&dyn rusqlite::ToSql> =
            active_roots.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        conn.execute(&sql, params_vec.as_slice())?;
        Ok(())
    }

    /// 部分一致検索 (大文字小文字無視)。結果は表示名で昇順ソート済み。
    /// `favorite_roots` が空の場合は全件対象。
    pub fn search(
        &self,
        query: &str,
        favorite_roots: &[PathBuf],
    ) -> rusqlite::Result<Vec<IndexEntry>> {
        let conn = self.conn.lock().unwrap();
        let q_lower = query.to_lowercase();
        let pattern = format!("%{}%", q_lower);

        let (sql, use_filter) = if favorite_roots.is_empty() {
            (
                "SELECT display_path, display_name, kind, mtime \
                 FROM entries \
                 WHERE name LIKE ?1 \
                 ORDER BY display_name COLLATE NOCASE \
                 LIMIT 5000"
                    .to_string(),
                false,
            )
        } else {
            let placeholders: Vec<String> = (2..=favorite_roots.len() + 1)
                .map(|i| format!("?{}", i))
                .collect();
            (
                format!(
                    "SELECT display_path, display_name, kind, mtime \
                     FROM entries \
                     WHERE name LIKE ?1 AND favorite_root IN ({}) \
                     ORDER BY display_name COLLATE NOCASE \
                     LIMIT 5000",
                    placeholders.join(",")
                ),
                true,
            )
        };

        let mut stmt = conn.prepare(&sql)?;
        let fav_norm_strs: Vec<String> = favorite_roots
            .iter()
            .map(|p| normalize_path(p))
            .collect();

        let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::new();
        params_vec.push(&pattern);
        if use_filter {
            for s in &fav_norm_strs {
                params_vec.push(s as &dyn rusqlite::ToSql);
            }
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
}

// -----------------------------------------------------------------------
// スキーマ
// -----------------------------------------------------------------------

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entries (
             path                  TEXT PRIMARY KEY,
             display_path          TEXT NOT NULL,
             name                  TEXT NOT NULL,
             display_name          TEXT NOT NULL,
             kind                  INTEGER NOT NULL,
             favorite_root         TEXT NOT NULL,
             mtime                 INTEGER NOT NULL DEFAULT 0,
             updated_at            INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS idx_entries_name ON entries(name);
         CREATE INDEX IF NOT EXISTS idx_entries_fav  ON entries(favorite_root);",
    )
}

fn chrono_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
        assert!(!is_under(
            Path::new(r"C:\Photos2"),
            Path::new(r"C:\Photos"),
        ));
        assert!(!is_under(Path::new(r"D:\Photos"), Path::new(r"C:\Photos")));
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

        let results = db.search("alp", &[]).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display_name, "alpha");

        let results = db.search(".zip", &[]).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, IndexKind::ZipFile);

        // 大文字小文字無視
        let results = db.search("BETA", &[]).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn upsert_replaces_siblings_only() {
        let db = open_mem();
        let fav = PathBuf::from(r"C:\Fav");
        let parent_a = PathBuf::from(r"C:\Fav\A");
        let parent_b = PathBuf::from(r"C:\Fav\B");

        db.upsert_children(&fav, &parent_a, &[
            entry(r"C:\Fav\A\x", "x", IndexKind::Folder),
            entry(r"C:\Fav\A\y", "y", IndexKind::Folder),
        ]).unwrap();
        db.upsert_children(&fav, &parent_b, &[
            entry(r"C:\Fav\B\z", "z", IndexKind::Folder),
        ]).unwrap();
        assert_eq!(db.total_count().unwrap(), 3);

        // A 配下を再 upsert (y を消して w を追加)、B は触らない
        db.upsert_children(&fav, &parent_a, &[
            entry(r"C:\Fav\A\x", "x", IndexKind::Folder),
            entry(r"C:\Fav\A\w", "w", IndexKind::Folder),
        ]).unwrap();
        let all = db.search("", &[]).unwrap();
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
        db.upsert_children(&fav1, &fav1, &[
            entry(r"C:\Fav1\a", "a", IndexKind::Folder),
        ]).unwrap();
        db.upsert_children(&fav2, &fav2, &[
            entry(r"C:\Fav2\b", "b", IndexKind::Folder),
        ]).unwrap();
        assert_eq!(db.total_count().unwrap(), 2);

        db.clear_for_favorite(&fav1).unwrap();
        assert_eq!(db.total_count().unwrap(), 1);
        let results = db.search("", &[]).unwrap();
        assert_eq!(results[0].display_name, "b");
    }

    #[test]
    fn search_filtered_by_favorite() {
        let db = open_mem();
        let fav1 = PathBuf::from(r"C:\Fav1");
        let fav2 = PathBuf::from(r"C:\Fav2");
        db.upsert_children(&fav1, &fav1, &[
            entry(r"C:\Fav1\match", "match", IndexKind::Folder),
        ]).unwrap();
        db.upsert_children(&fav2, &fav2, &[
            entry(r"C:\Fav2\match", "match", IndexKind::Folder),
        ]).unwrap();
        let results = db.search("match", &[fav1.clone()]).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, PathBuf::from(r"C:\Fav1\match"));
    }

    #[test]
    fn prune_obsolete() {
        let db = open_mem();
        let fav_keep = PathBuf::from(r"C:\Keep");
        let fav_drop = PathBuf::from(r"C:\Drop");
        db.upsert_children(&fav_keep, &fav_keep, &[
            entry(r"C:\Keep\a", "a", IndexKind::Folder),
        ]).unwrap();
        db.upsert_children(&fav_drop, &fav_drop, &[
            entry(r"C:\Drop\b", "b", IndexKind::Folder),
        ]).unwrap();
        assert_eq!(db.total_count().unwrap(), 2);
        db.prune_obsolete(&[normalize_path(&fav_keep)]).unwrap();
        assert_eq!(db.total_count().unwrap(), 1);
    }
}
