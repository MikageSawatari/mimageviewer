//! `fts_meta.db` — 全文メタ検索のファイル単位メタ管理 + post-filter 用原文保存。
//!
//! docs/search-expansion-design.md §5.3, §5.6, §19.4 に準拠する。
//!
//! Tantivy インデックス (`fts_index/`) とは別 DB で、以下を担う:
//! - お気に入り単位の登録ファイル追跡 (差分検出の基準)
//! - ソース別正規化テキストの保存 (Ctrl+G post-filter で target に応じた結合を行う)
//! - `status=pending / ok / failed / tombstone` の二段整合性状態
//! - ingest 世代カウンタ (`index_generation`) — 将来のスナップショット用
//!
//! **スレッド安全性**: `Mutex<Connection>` で包む (既存 catalog.rs と同じパターン)。
//! 頻繁な UPSERT は Ingest Worker から呼ばれるため、ロックは短く保つ。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::fts_index::{IndexKind, SearchTarget};
use crate::ingest_text::PerSourceText;
use crate::search_index_db::normalize_path;

/// スキーマ変更時に bump することで、次回起動時に全再インデックスをトリガする定数。
/// v3 (§19 + tag 統合): per-source 5 カラム + kind + tags_norm の 16 列スキーマ。
pub const INDEX_VERSION: i64 = 3;

/// 1 ファイル/ZIP エントリに対応する fts_meta.db の行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMeta {
    /// 正規化済みパス (lowercase + `/`、ドライブレター保持、ZIP 内は `<zip>!<entry>`)
    pub path: String,
    pub favorite_id: Uuid,
    pub favorite_root: PathBuf,
    pub kind: IndexKind,
    pub mtime: i64,
    pub file_size: i64,
    pub indexed_at: i64,
    pub index_version: i64,
    pub index_generation: i64,
    pub status: FileStatus,
    /// `search_norm::normalize_for_match` 適用済み per-source テキスト + tags。
    pub norms: PerSourceText,
}

impl FileMeta {
    /// post-filter 用: 選択された検索対象をスペース区切りで結合した文字列を返す。
    pub fn combined_for_target(&self, target: &SearchTarget) -> String {
        let mut out = String::new();
        for &src in target.sources() {
            let s = self.norms.get(src);
            if s.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(s);
        }
        out
    }
}

/// ingest と delete の二段整合性プロトコル用 (§5.6)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum FileStatus {
    /// Tantivy にコミット済み、検索可能
    Ok = 0,
    /// ingest 開始済み、Tantivy へのコミット待ち / 進行中
    Pending = 1,
    /// ingest 失敗 (次回再試行)
    Failed = 2,
    /// 削除予定 (Tantivy からの delete を待つ間、post-filter で除外)
    Tombstone = 3,
}

impl FileStatus {
    fn from_i64(v: i64) -> Self {
        match v {
            0 => Self::Ok,
            1 => Self::Pending,
            2 => Self::Failed,
            3 => Self::Tombstone,
            _ => Self::Failed, // 不正値は failed 扱いで再試行
        }
    }
}

/// fts_meta.db への接続。
pub struct FtsMetaDb {
    conn: Mutex<Connection>,
}

impl FtsMetaDb {
    /// `%APPDATA%/mimageviewer/fts_meta.db` を開く (なければ作成)。
    pub fn open() -> rusqlite::Result<Self> {
        let db_path = crate::data_dir::get().join("fts_meta.db");
        Self::open_at(&db_path)
    }

    /// 任意パスに開く (テスト用)。
    pub fn open_at(db_path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )?;
        // §19.8 自動マイグレーション: 旧スキーマを検出したら files を drop して再作成する。
        // (Tantivy 側は FtsIndex::open_at が並列で wipe する)
        //
        // Codex startup 計装で fts_meta.db (~2GB) に対する `SELECT MIN(index_version)` が
        // cold cache で 10 秒超フルスキャンしていた。PRAGMA user_version でスキーマ最新
        // フラグを持ち、一致していればスキャンを完全に回避する。
        // - user_version == INDEX_VERSION: 最新 → rebuild 不要、MIN スキャンなし
        // - それ以外: 旧来の needs_rebuild() ロジックを実行 (初回起動 / 旧 DB)
        let user_version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let rebuild_needed = if user_version == INDEX_VERSION {
            false
        } else {
            needs_rebuild(&conn)?
        };
        if rebuild_needed {
            crate::logger::log(
                "fts_meta: detected old schema (INDEX_VERSION < 2) — dropping `files` table for rebuild",
            );
            conn.execute_batch("DROP TABLE IF EXISTS files;")?;
        }
        init_schema(&conn)?;
        // スキーマ最新を user_version に記録。次回起動時の MIN スキャンを回避する。
        // `PRAGMA user_version = N` はパラメータバインド不可なので format! で組み立てる
        // (INDEX_VERSION は const なので SQL injection リスクはない)。
        if user_version != INDEX_VERSION {
            conn.execute_batch(&format!("PRAGMA user_version = {INDEX_VERSION};"))?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// status=pending で UPSERT。既存 row の generation を増やし、ソース別正規化テキストを更新する。
    /// Tantivy への add_document 前に呼ばれる (§5.6.1 ステップ 1 / §19.4 / tag 統合)。
    pub fn mark_pending(
        &self,
        path: &str,
        favorite_id: Uuid,
        favorite_root: &Path,
        kind: IndexKind,
        mtime: i64,
        file_size: i64,
        norms: &PerSourceText,
    ) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = now_epoch();
        conn.execute(
            "INSERT INTO files (
                path, favorite_id, favorite_root, kind, mtime, file_size,
                indexed_at, index_version, index_generation, status,
                name_norm, exif_norm, xmp_tweet_norm, png_prompt_norm, pdf_meta_norm, tags_norm
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(path) DO UPDATE SET
                favorite_id = excluded.favorite_id,
                favorite_root = excluded.favorite_root,
                kind = excluded.kind,
                mtime = excluded.mtime,
                file_size = excluded.file_size,
                indexed_at = excluded.indexed_at,
                index_version = excluded.index_version,
                index_generation = files.index_generation + 1,
                status = 1,
                name_norm = excluded.name_norm,
                exif_norm = excluded.exif_norm,
                xmp_tweet_norm = excluded.xmp_tweet_norm,
                png_prompt_norm = excluded.png_prompt_norm,
                pdf_meta_norm = excluded.pdf_meta_norm,
                tags_norm = excluded.tags_norm",
            params![
                path,
                favorite_id.to_string(),
                favorite_root.to_string_lossy().into_owned(),
                kind.to_i64(),
                mtime,
                file_size,
                now,
                INDEX_VERSION,
                FileStatus::Pending as i64,
                norms.name,
                norms.exif,
                norms.xmp_tweet,
                norms.png_prompt,
                norms.pdf_meta,
                norms.tags,
            ],
        )?;
        let gen_val: i64 = conn.query_row(
            "SELECT index_generation FROM files WHERE path = ?1",
            params![path],
            |r| r.get(0),
        )?;
        Ok(gen_val)
    }

    /// post-filter 用: 指定 path 群のタグ列を一括取得。
    /// status=Ok のみ返す (`lookup_norms_for_target` と同じポリシー)。
    /// タグ書き込み worker が ingest 待たずに fts_meta を直接更新するときにも使う。
    pub fn lookup_tags(&self, paths: &[String]) -> rusqlite::Result<Vec<(String, String)>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = (0..paths.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT path, tags_norm FROM files \
             WHERE path IN ({}) AND status = 0",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_vec: Vec<&dyn rusqlite::ToSql> =
            paths.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::with_capacity(paths.len());
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 単一 path のタグ更新 (タグ書き込み worker が呼ぶ高速経路)。
    pub fn set_tags(&self, path: &str, tags: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE files SET tags_norm = ?1 WHERE path = ?2",
            params![tags, path],
        )?;
        Ok(())
    }

    /// Tantivy commit 後に呼ぶ。status=pending → ok に遷移 (§5.6.1 ステップ 4)。
    pub fn mark_ok(&self, paths: &[String]) -> rusqlite::Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt =
                tx.prepare("UPDATE files SET status = 0 WHERE path = ?1 AND status = 1")?;
            for p in paths {
                stmt.execute(params![p])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// ingest 失敗を記録。次回再試行 (retry 抑制は上位レイヤーで管理)。
    pub fn mark_failed(&self, path: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE files SET status = 2 WHERE path = ?1", params![path])?;
        Ok(())
    }

    /// お気に入り配下の全行を tombstone にする (favorite の「メタ」チェックを OFF にした時)。
    ///
    /// 返り値は tombstone に変えた行数。実際の Tantivy 削除は次回起動時の
    /// reconciliation が処理する (status=3 の行は tombstone_purged 経由で delete_doc される)。
    /// Ctrl+F / Ctrl+G の post-filter は `lookup_all_text_norm` で status=0 のみ見るので、
    /// この場で tombstone にしておけば検索結果から即座に消える。
    pub fn mark_tombstone_all_for_favorite(&self, favorite_id: Uuid) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE files SET status = 3 WHERE favorite_id = ?1",
            params![favorite_id.to_string()],
        )?;
        Ok(changed)
    }

    /// 削除開始: status → tombstone (§5.6.2 ステップ 1)。
    /// post-filter 時の除外対象になる。Tantivy delete 後に `purge_tombstone` で完全削除。
    pub fn mark_tombstone(&self, paths: &[String]) -> rusqlite::Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("UPDATE files SET status = 3 WHERE path = ?1")?;
            for p in paths {
                stmt.execute(params![p])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Tantivy からの delete が commit された後に呼ぶ (§5.6.2 ステップ 4)。
    /// tombstone 状態の行を物理削除する。
    pub fn purge_tombstone(&self, paths: &[String]) -> rusqlite::Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let mut deleted = 0;
        {
            let mut stmt = tx.prepare("DELETE FROM files WHERE path = ?1 AND status = 3")?;
            for p in paths {
                deleted += stmt.execute(params![p])?;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// post-filter 用: 指定 path 群の「対象ソースを結合した正規化テキスト」を一括取得する (§19.4)。
    ///
    /// **`status = 0 (Ok)` のみを返す** (Codex 6 回目指摘 #5)。
    /// pending / failed / tombstone は除外:
    ///   - pending: ingest 進行中でテキストが新しいが Tantivy 側は古い snapshot →
    ///     二段整合性を保つため検索結果から外す
    ///   - failed: メタ抽出失敗で不完全な可能性
    ///   - tombstone: 削除済み (Tantivy 側は commit 待ち)
    ///
    /// `target` で選択されたソース列だけを結合する。`SearchTarget::All` は 5 列全部。
    /// 戻り値は入力 path 順不同の `Vec<(path, combined_norm_text)>`。
    pub fn lookup_norms_for_target(
        &self,
        paths: &[String],
        target: &SearchTarget,
    ) -> rusqlite::Result<Vec<(String, String)>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = (0..paths.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        // 全列を SELECT して Rust 側で結合する (target による動的 SQL の複雑さを避ける)。
        let sql = format!(
            "SELECT path, name_norm, exif_norm, xmp_tweet_norm, png_prompt_norm, pdf_meta_norm, tags_norm \
             FROM files WHERE path IN ({}) AND status = 0",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_vec: Vec<&dyn rusqlite::ToSql> =
            paths.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let sources = target.sources();
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), |row| {
            let path: String = row.get(0)?;
            let norms = PerSourceText {
                name: row.get(1)?,
                exif: row.get(2)?,
                xmp_tweet: row.get(3)?,
                png_prompt: row.get(4)?,
                pdf_meta: row.get(5)?,
                tags: row.get(6)?,
            };
            let mut out = String::new();
            for &src in sources {
                let s = norms.get(src);
                if s.is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(s);
            }
            Ok((path, out))
        })?;
        let mut out = Vec::with_capacity(paths.len());
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 起動時差分走査 (§7.4) で使用。favorite_id スコープ内の **status=Ok のファイルのみ**
    /// (path, mtime, file_size) を返す (Codex 6 回目指摘 #6)。
    ///
    /// pending / failed / tombstone を "既存" として扱わないので、
    /// クラッシュで残った pending はこの結果に入らず、差分 diff が
    /// 「FS にあるけど DB に無い」と判定して再 ingest に回す。
    /// tombstone も除外するので、削除保留の path が "まだある" 扱いにはならない。
    ///
    /// Supervisor 起動前に `reconcile_not_ok_paths()` を呼ぶことで、
    /// status != 0 の path 一覧を取り別途再 ingest キューに乗せられる (§5.6.3)。
    pub fn list_favorite_files(
        &self,
        favorite_id: Uuid,
    ) -> rusqlite::Result<Vec<(String, i64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT path, mtime, file_size FROM files \
             WHERE favorite_id = ?1 AND status = 0",
        )?;
        let rows = stmt.query_map(params![favorite_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 起動時 reconciliation 用 (§5.6.3, Codex 6 回目指摘 #6):
    /// 指定お気に入りスコープで status != Ok の path 一覧を返す。
    /// 呼び出し側は:
    ///   - pending / failed: 再 ingest キューへ
    ///   - tombstone: Tantivy delete 再実行 → purge
    /// で復旧する。
    pub fn list_not_ok_paths(
        &self,
        favorite_id: Uuid,
    ) -> rusqlite::Result<Vec<(String, FileStatus)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT path, status FROM files \
             WHERE favorite_id = ?1 AND status != 0",
        )?;
        let rows = stmt.query_map(params![favorite_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (p, s) = row?;
            out.push((p, FileStatus::from_i64(s)));
        }
        Ok(out)
    }

    /// 起動時 reconciliation (§5.6.3) 用。status != 0 の行を全部取る。
    pub fn list_not_ok(&self) -> rusqlite::Result<Vec<FileMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(FILEMETA_SELECT_SQL_NOT_OK)?;
        let rows = stmt.query_map([], row_to_filemeta)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// テスト / デバッグ用: 単一 path の行を取得。
    pub fn get(&self, path: &str) -> rusqlite::Result<Option<FileMeta>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(FILEMETA_SELECT_SQL_BY_PATH, params![path], row_to_filemeta)
            .optional()
    }

    /// favorite_id でフィルタしつつ総数と status 別の件数を返す。
    /// インデックス管理ダイアログの進捗表示 (§8.4) で使用。
    pub fn count_by_status(&self, favorite_id: Uuid) -> rusqlite::Result<StatusCounts> {
        let conn = self.conn.lock().unwrap();
        let mut counts = StatusCounts::default();
        let mut stmt = conn
            .prepare("SELECT status, COUNT(*) FROM files WHERE favorite_id = ?1 GROUP BY status")?;
        let rows = stmt.query_map(params![favorite_id.to_string()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (s, c) = row?;
            match FileStatus::from_i64(s) {
                FileStatus::Ok => counts.ok = c as usize,
                FileStatus::Pending => counts.pending = c as usize,
                FileStatus::Failed => counts.failed = c as usize,
                FileStatus::Tombstone => counts.tombstone = c as usize,
            }
        }
        Ok(counts)
    }

    /// テスト用: ファイル名キー (normalize_path(&PathBuf) 結果) の key 生成ヘルパー。
    pub fn path_key(p: &Path) -> String {
        normalize_path(p)
    }
}

// `row_to_filemeta` が受け取るカラム順。2 つの SELECT で使い回す。
// concat! は literal しか受けないので、列リスト部分をマクロで共有する。
macro_rules! filemeta_select_cols {
    () => {
        "path, favorite_id, favorite_root, kind, mtime, file_size, \
         indexed_at, index_version, index_generation, status, \
         name_norm, exif_norm, xmp_tweet_norm, png_prompt_norm, pdf_meta_norm, tags_norm"
    };
}

const FILEMETA_SELECT_SQL_NOT_OK: &str =
    concat!("SELECT ", filemeta_select_cols!(), " FROM files WHERE status != 0");

const FILEMETA_SELECT_SQL_BY_PATH: &str =
    concat!("SELECT ", filemeta_select_cols!(), " FROM files WHERE path = ?1");

fn row_to_filemeta(row: &rusqlite::Row) -> rusqlite::Result<FileMeta> {
    let uuid_str: String = row.get(1)?;
    let favorite_id = Uuid::parse_str(&uuid_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let favorite_root: String = row.get(2)?;
    let kind_i: i64 = row.get(3)?;
    let status_i: i64 = row.get(9)?;
    Ok(FileMeta {
        path: row.get(0)?,
        favorite_id,
        favorite_root: PathBuf::from(favorite_root),
        kind: IndexKind::from_i64(kind_i),
        mtime: row.get(4)?,
        file_size: row.get(5)?,
        indexed_at: row.get(6)?,
        index_version: row.get(7)?,
        index_generation: row.get(8)?,
        status: FileStatus::from_i64(status_i),
        norms: PerSourceText {
            name: row.get(10)?,
            exif: row.get(11)?,
            xmp_tweet: row.get(12)?,
            png_prompt: row.get(13)?,
            pdf_meta: row.get(14)?,
            tags: row.get(15)?,
        },
    })
}

/// 既存の `files` テーブルが旧スキーマ (INDEX_VERSION=1 時代) かを判定する (§19.8)。
///
/// 判定方針: `files` が存在し、かつ以下のいずれかに該当すれば旧版:
/// - `all_text_norm` カラムが残っている (v1 → v2 移行前)
/// - `index_version` 列の MIN 値が `INDEX_VERSION` より小さい (将来的な v2 → v3 移行用)
///
/// テーブルが存在しなければ `false` (新規作成なので rebuild 不要)。
fn needs_rebuild(conn: &Connection) -> rusqlite::Result<bool> {
    // files テーブル自体が無い新規 DB なら rebuild 不要
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='files'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !has_table {
        return Ok(false);
    }
    // 旧カラム検出: name_norm が欠ける / tags_norm が欠ける / all_text_norm が残っている場合は rebuild
    let mut stmt = conn.prepare("PRAGMA table_info(files)")?;
    let mut has_all_text_norm = false;
    let mut has_name_norm = false;
    let mut has_tags_norm = false;
    let rows = stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        Ok(name)
    })?;
    for r in rows {
        let name = r?;
        match name.as_str() {
            "all_text_norm" => has_all_text_norm = true,
            "name_norm" => has_name_norm = true,
            "tags_norm" => has_tags_norm = true,
            _ => {}
        }
    }
    drop(stmt);
    if has_all_text_norm || !has_name_norm || !has_tags_norm {
        return Ok(true);
    }
    // index_version が古い行が残っている (過去にこの DB で別バージョンを使っていた等)。
    // MIN は空テーブルなら NULL 行を返すため Option<i64> で受ける
    // (`.optional()` は `QueryReturnedNoRows` 用で、MIN の NULL 行はこれに当たらない)。
    let min_ver: Option<i64> = conn
        .query_row("SELECT MIN(index_version) FROM files", [], |row| {
            row.get::<_, Option<i64>>(0)
        })?;
    Ok(matches!(min_ver, Some(v) if v < INDEX_VERSION))
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS files (
            path              TEXT PRIMARY KEY,
            favorite_id       TEXT NOT NULL,
            favorite_root     TEXT NOT NULL,
            kind              INTEGER NOT NULL DEFAULT 1,
            mtime             INTEGER NOT NULL,
            file_size         INTEGER NOT NULL,
            indexed_at        INTEGER NOT NULL,
            index_version     INTEGER NOT NULL,
            index_generation  INTEGER NOT NULL,
            status            INTEGER NOT NULL,
            name_norm         TEXT NOT NULL DEFAULT '',
            exif_norm         TEXT NOT NULL DEFAULT '',
            xmp_tweet_norm    TEXT NOT NULL DEFAULT '',
            png_prompt_norm   TEXT NOT NULL DEFAULT '',
            pdf_meta_norm     TEXT NOT NULL DEFAULT '',
            tags_norm         TEXT NOT NULL DEFAULT ''
         );
         CREATE INDEX IF NOT EXISTS idx_files_fav       ON files(favorite_id);
         CREATE INDEX IF NOT EXISTS idx_files_fav_mtime ON files(favorite_id, mtime);
         CREATE INDEX IF NOT EXISTS idx_files_fav_kind  ON files(favorite_id, kind);
         CREATE INDEX IF NOT EXISTS idx_files_status    ON files(status) WHERE status != 0;",
    )?;
    Ok(())
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// favorite 単位の status 別カウント (§8.4 の UI 表示で使用)。
#[derive(Default, Debug, Clone, Copy)]
pub struct StatusCounts {
    pub ok: usize,
    pub pending: usize,
    pub failed: usize,
    pub tombstone: usize,
}

impl StatusCounts {
    pub fn total(self) -> usize {
        self.ok + self.pending + self.failed + self.tombstone
    }
    /// インデックスされた有効件数 (検索結果に含まれうる)
    pub fn indexed(self) -> usize {
        self.ok
    }
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fts_index::SourceKind;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tmp_db() -> (TempDir, FtsMetaDb) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        let db = FtsMetaDb::open_at(&db_path).unwrap();
        (dir, db)
    }

    fn norms_named(name: &str) -> PerSourceText {
        PerSourceText {
            name: name.to_string(),
            ..PerSourceText::default()
        }
    }

    #[test]
    fn create_and_list_empty() {
        let (_tmp, db) = tmp_db();
        let id = Uuid::new_v4();
        assert!(db.list_favorite_files(id).unwrap().is_empty());
        assert!(db.list_not_ok().unwrap().is_empty());
    }

    /// 新規 DB 作成後に `PRAGMA user_version` が `INDEX_VERSION` と一致すること。
    /// これにより次回 open 時は needs_rebuild の MIN スキャンがスキップされる (起動高速化)。
    #[test]
    fn open_sets_user_version_to_index_version() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        {
            let _db = FtsMetaDb::open_at(&db_path).unwrap();
        } // close
        // 直接 SQLite を開いて user_version を読む
        let conn = Connection::open(&db_path).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, INDEX_VERSION);
    }

    /// user_version が古ければ MIN スキャン経路が走り、rebuild 判定が機能すること。
    #[test]
    fn legacy_db_without_user_version_triggers_check() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        // 既存スキーマの DB を用意して user_version を古くしておく
        {
            let _db = FtsMetaDb::open_at(&db_path).unwrap();
        }
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA user_version = 0;").unwrap();
        }
        // もう一度 open → user_version が古いので check 経路を通るが、
        // 実スキーマは最新なので rebuild はされず、最終的に user_version は INDEX_VERSION に戻る
        {
            let _db = FtsMetaDb::open_at(&db_path).unwrap();
        }
        let conn = Connection::open(&db_path).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, INDEX_VERSION);
    }

    #[test]
    fn pending_then_ok_roundtrip() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/photos");
        let n1 = norms_named("text a");
        let gen1 = db
            .mark_pending(
                "c:/photos/a.jpg",
                fav,
                &root,
                IndexKind::Image,
                100,
                2048,
                &n1,
            )
            .unwrap();
        assert_eq!(gen1, 1, "初回 ingest の generation は 1");

        // 状態: pending
        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Pending);
        assert_eq!(got.norms.name, "text a");
        assert_eq!(got.kind, IndexKind::Image);
        assert_eq!(got.index_generation, 1);
        assert_eq!(got.favorite_id, fav);

        // ok に遷移
        db.mark_ok(&["c:/photos/a.jpg".to_string()]).unwrap();
        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Ok);

        // 同じ path で再 ingest → generation += 1
        let n2 = norms_named("text a updated");
        let gen2 = db
            .mark_pending(
                "c:/photos/a.jpg",
                fav,
                &root,
                IndexKind::Image,
                200,
                2100,
                &n2,
            )
            .unwrap();
        assert_eq!(gen2, 2);
        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Pending);
        assert_eq!(got.norms.name, "text a updated");
    }

    #[test]
    fn tombstone_hides_from_lookup() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/p");
        db.mark_pending(
            "c:/p/1.jpg",
            fav,
            &root,
            IndexKind::Image,
            1,
            10,
            &norms_named("one"),
        )
        .unwrap();
        db.mark_pending(
            "c:/p/2.jpg",
            fav,
            &root,
            IndexKind::Image,
            2,
            20,
            &norms_named("two"),
        )
        .unwrap();
        db.mark_ok(&["c:/p/1.jpg".to_string(), "c:/p/2.jpg".to_string()])
            .unwrap();

        // lookup で両方返る
        let rows = db
            .lookup_norms_for_target(
                &["c:/p/1.jpg".to_string(), "c:/p/2.jpg".to_string()],
                &SearchTarget::All,
            )
            .unwrap();
        assert_eq!(rows.len(), 2);

        // tombstone 後は除外される
        db.mark_tombstone(&["c:/p/1.jpg".to_string()]).unwrap();
        let rows = db
            .lookup_norms_for_target(
                &["c:/p/1.jpg".to_string(), "c:/p/2.jpg".to_string()],
                &SearchTarget::All,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "c:/p/2.jpg");

        // purge で物理削除
        let deleted = db.purge_tombstone(&["c:/p/1.jpg".to_string()]).unwrap();
        assert_eq!(deleted, 1);
        assert!(db.get("c:/p/1.jpg").unwrap().is_none());
    }

    #[test]
    fn list_favorite_files_returns_only_ok() {
        let (_tmp, db) = tmp_db();
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let root_a = PathBuf::from("C:/a");
        let root_b = PathBuf::from("C:/b");
        let empty = PerSourceText::default();
        db.mark_pending("c:/a/1.jpg", fav_a, &root_a, IndexKind::Image, 1, 1, &empty)
            .unwrap();
        db.mark_pending("c:/a/2.jpg", fav_a, &root_a, IndexKind::Image, 2, 2, &empty)
            .unwrap();
        db.mark_pending("c:/b/1.jpg", fav_b, &root_b, IndexKind::Image, 3, 3, &empty)
            .unwrap();

        // 全部 pending のまま → list_favorite_files には出てこない
        assert!(db.list_favorite_files(fav_a).unwrap().is_empty());
        assert!(db.list_favorite_files(fav_b).unwrap().is_empty());

        // ok に遷移したもののみ返る
        db.mark_ok(&["c:/a/1.jpg".to_string(), "c:/a/2.jpg".to_string()])
            .unwrap();
        let a = db.list_favorite_files(fav_a).unwrap();
        assert_eq!(a.len(), 2);
        assert!(db.list_favorite_files(fav_b).unwrap().is_empty());

        let not_ok = db.list_not_ok_paths(fav_b).unwrap();
        assert_eq!(not_ok.len(), 1);
        assert_eq!(not_ok[0].0, "c:/b/1.jpg");
        assert_eq!(not_ok[0].1, FileStatus::Pending);
    }

    #[test]
    fn lookup_norms_for_target_returns_only_ok() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/p");
        db.mark_pending(
            "c:/p/a.jpg",
            fav,
            &root,
            IndexKind::Image,
            1,
            1,
            &norms_named("alpha"),
        )
        .unwrap();
        db.mark_pending(
            "c:/p/b.jpg",
            fav,
            &root,
            IndexKind::Image,
            2,
            2,
            &norms_named("beta"),
        )
        .unwrap();

        // 両方 pending → lookup は空
        let rows = db
            .lookup_norms_for_target(
                &["c:/p/a.jpg".to_string(), "c:/p/b.jpg".to_string()],
                &SearchTarget::All,
            )
            .unwrap();
        assert!(rows.is_empty(), "pending は post-filter に含めない");

        // ok に遷移 → 返る
        db.mark_ok(&["c:/p/a.jpg".to_string()]).unwrap();
        let rows = db
            .lookup_norms_for_target(
                &["c:/p/a.jpg".to_string(), "c:/p/b.jpg".to_string()],
                &SearchTarget::All,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "c:/p/a.jpg");
        assert_eq!(rows[0].1, "alpha");

        // failed に遷移 → 除外
        db.mark_failed("c:/p/a.jpg").unwrap();
        let rows = db
            .lookup_norms_for_target(&["c:/p/a.jpg".to_string()], &SearchTarget::All)
            .unwrap();
        assert!(rows.is_empty(), "failed も post-filter から除外");
    }

    #[test]
    fn lookup_norms_for_target_selects_requested_sources() {
        // §19.6: Only(&[Exif]) では exif_norm 列のみが結合対象になる。
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/p");
        let norms = PerSourceText {
            name: "file.jpg".into(),
            exif: "canon exif".into(),
            xmp_tweet: "tweet body".into(),
            png_prompt: "".into(),
            pdf_meta: "".into(),
            tags: "".into(),
        };
        db.mark_pending(
            "c:/p/a.jpg",
            fav,
            &root,
            IndexKind::Image,
            1,
            1,
            &norms,
        )
        .unwrap();
        db.mark_ok(&["c:/p/a.jpg".to_string()]).unwrap();

        // Only(Exif) → "canon exif" のみ
        let rows = db
            .lookup_norms_for_target(
                &["c:/p/a.jpg".to_string()],
                &SearchTarget::Only(vec![SourceKind::Exif]),
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "canon exif");

        // Only(XmpTweet) → "tweet body" のみ
        let rows = db
            .lookup_norms_for_target(
                &["c:/p/a.jpg".to_string()],
                &SearchTarget::Only(vec![SourceKind::XmpTweet]),
            )
            .unwrap();
        assert_eq!(rows[0].1, "tweet body");

        // All → 全ソース結合 (空列はスキップ)
        let rows = db
            .lookup_norms_for_target(&["c:/p/a.jpg".to_string()], &SearchTarget::All)
            .unwrap();
        assert_eq!(rows[0].1, "file.jpg canon exif tweet body");
    }

    #[test]
    fn list_not_ok_returns_pending_and_failed() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/x");
        let empty = PerSourceText::default();
        db.mark_pending("c:/x/1.jpg", fav, &root, IndexKind::Image, 1, 1, &empty)
            .unwrap();
        db.mark_pending("c:/x/2.jpg", fav, &root, IndexKind::Image, 2, 2, &empty)
            .unwrap();
        db.mark_pending("c:/x/3.jpg", fav, &root, IndexKind::Image, 3, 3, &empty)
            .unwrap();

        db.mark_ok(&["c:/x/1.jpg".to_string()]).unwrap();
        db.mark_failed("c:/x/2.jpg").unwrap();

        let not_ok = db.list_not_ok().unwrap();
        assert_eq!(not_ok.len(), 2);
        let statuses: Vec<_> = not_ok.iter().map(|m| m.status).collect();
        assert!(statuses.contains(&FileStatus::Pending));
        assert!(statuses.contains(&FileStatus::Failed));
    }

    #[test]
    fn count_by_status_groups() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/y");
        let empty = PerSourceText::default();
        for i in 0..5 {
            db.mark_pending(
                &format!("c:/y/{}.jpg", i),
                fav,
                &root,
                IndexKind::Image,
                i,
                1,
                &empty,
            )
            .unwrap();
        }
        db.mark_ok(&["c:/y/0.jpg".to_string(), "c:/y/1.jpg".to_string()])
            .unwrap();
        db.mark_failed("c:/y/2.jpg").unwrap();
        db.mark_tombstone(&["c:/y/3.jpg".to_string()]).unwrap();

        let c = db.count_by_status(fav).unwrap();
        assert_eq!(c.ok, 2);
        assert_eq!(c.pending, 1);
        assert_eq!(c.failed, 1);
        assert_eq!(c.tombstone, 1);
        assert_eq!(c.total(), 5);
        assert_eq!(c.indexed(), 2);
    }

    #[test]
    fn opening_old_schema_db_drops_and_rebuilds() {
        // §19.8 マイグレーション回帰: 旧 v1 スキーマの DB を開いたら自動で drop → recreate する。
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");

        // 旧スキーマの files テーブルを手動で作る (all_text_norm カラム有、name_norm 無)
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE files (
                    path TEXT PRIMARY KEY,
                    favorite_id TEXT NOT NULL,
                    favorite_root TEXT NOT NULL,
                    mtime INTEGER NOT NULL,
                    file_size INTEGER NOT NULL,
                    indexed_at INTEGER NOT NULL,
                    index_version INTEGER NOT NULL,
                    index_generation INTEGER NOT NULL,
                    status INTEGER NOT NULL,
                    all_text_norm TEXT NOT NULL DEFAULT ''
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO files (path, favorite_id, favorite_root, mtime, file_size,
                 indexed_at, index_version, index_generation, status, all_text_norm)
                 VALUES ('c:/old.jpg', ?1, 'C:/', 1, 1, 1, 1, 1, 0, 'legacy')",
                params![Uuid::new_v4().to_string()],
            )
            .unwrap();
        }

        // open_at が new schema で再構築する
        let db = FtsMetaDb::open_at(&db_path).unwrap();

        // 旧行は消えている
        assert!(db.get("c:/old.jpg").unwrap().is_none(), "旧データは消えた");

        // 新しい mark_pending が通る
        let fav = Uuid::new_v4();
        db.mark_pending(
            "c:/new.jpg",
            fav,
            std::path::Path::new("C:/"),
            IndexKind::Image,
            1,
            1,
            &norms_named("new"),
        )
        .unwrap();
        let row = db.get("c:/new.jpg").unwrap().unwrap();
        assert_eq!(row.norms.name, "new");
    }

    #[test]
    fn opening_fresh_db_does_not_trigger_rebuild() {
        // 新規 DB (files テーブルすら無い) では rebuild パスに入らず、そのまま init_schema が走る。
        let dir = TempDir::new().unwrap();
        let db = FtsMetaDb::open_at(&dir.path().join("fresh.db")).unwrap();
        assert!(db.list_not_ok().unwrap().is_empty());
    }

    #[test]
    fn lookup_returns_only_matching_paths() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/z");
        db.mark_pending(
            "c:/z/a.jpg",
            fav,
            &root,
            IndexKind::Image,
            1,
            1,
            &norms_named("alpha"),
        )
        .unwrap();
        db.mark_pending(
            "c:/z/b.jpg",
            fav,
            &root,
            IndexKind::Image,
            2,
            2,
            &norms_named("beta"),
        )
        .unwrap();
        db.mark_ok(&["c:/z/a.jpg".to_string(), "c:/z/b.jpg".to_string()])
            .unwrap();

        // 存在しない path を含むクエリでも、存在するものだけ返る
        let rows = db
            .lookup_norms_for_target(
                &["c:/z/a.jpg".to_string(), "c:/z/missing.jpg".to_string()],
                &SearchTarget::All,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "c:/z/a.jpg");
        assert_eq!(rows[0].1, "alpha");
    }
}
