//! `fts_meta.db` — 全文メタ検索のファイル単位 **管理メタ** 専用 DB (INDEX_VERSION=5)。
//!
//! docs/search-architecture.md に準拠する。
//!
//! Tantivy インデックス (`fts_index/`) とは別 DB で、以下を担う:
//! - お気に入り単位の登録ファイル追跡 (差分検出の基準)
//! - `status=pending / ok / failed / tombstone` の二段整合性状態
//! - ingest 世代カウンタ (`index_generation`) — 将来のスナップショット用
//!
//! **正規化済み原文 (`*_norm` 列) は持たない**。INDEX_VERSION=5 で各ソースの
//! 原文は Tantivy 側 (`*_text` フィールドに STORED) に集約された。post-filter は
//! `fts_index::doc_text_for_target` で Tantivy snapshot から原文を取り出す経路に
//! 統一されている。
//!
//! **スレッド安全性**: `Mutex<Connection>` で包む (既存 catalog.rs と同じパターン)。
//! 頻繁な UPSERT は Ingest Worker から呼ばれるため、ロックは短く保つ。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::fts_index::IndexKind;
use crate::search_index_db::normalize_path;

/// スキーマ変更時に bump することで、次回起動時に全再インデックスをトリガする定数。
/// 全文検索インデックスのスキーマ / キー形式のバージョン。
///
/// ## bump 履歴
/// - 2: 多ソーステキスト (exif / xmp / png_prompt / pdf_meta に分割)
/// - 3: tags (XMP dc:subject) フィールド追加
/// - 4: ZIP 内エントリのキー separator を `!` から U+001F に変更
/// - 5: post-filter 用の正規化済み原文を Tantivy 側 (`*_text` フィールドに STORED)
///      へ集約。fts_meta.db の `*_norm` 列を撤去 (本ファイルから DDL も削除)。
///      これにより SQLite サイズが大幅縮小し、WAL checkpoint の負荷が下がる。
pub const INDEX_VERSION: i64 = 5;

/// 1 ファイル/ZIP エントリに対応する fts_meta.db の行。
///
/// INDEX_VERSION=5 以降は post-filter 用の原文 (`PerSourceText`) は Tantivy 側に
/// 持っており、ここでは管理メタ (status, mtime, generation 等) のみを保持する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMeta {
    /// 正規化済みパス (lowercase + `/`、ドライブレター保持、ZIP 内は
    /// `<zip>\u{1F}<entry>` 形式。separator は [`crate::search_norm::ZIP_ENTRY_SEP`])。
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
    /// この open で `files` テーブルが drop → 再作成された (スキーマ古い / INDEX_VERSION 不一致)。
    /// `true` なら Tantivy 側インデックスも wipe すべき (古い separator / 古い key 形式で
    /// 作られた Tantivy docs を残すと orphan として残留するため)。
    rebuilt_on_open: bool,
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
            crate::logger::log(format!(
                "fts_meta: detected old schema (index_version < {INDEX_VERSION}) — dropping `files` table for rebuild"
            ));
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
            rebuilt_on_open: rebuild_needed,
        })
    }

    /// 直近の `open_at` で `files` テーブルが再作成されたか。
    /// IndexerManager はこのフラグを見て Tantivy index dir を wipe する
    /// (旧 key 形式の orphan doc が残らないようにするため)。
    pub fn rebuilt_on_open(&self) -> bool {
        self.rebuilt_on_open
    }

    /// status=pending で UPSERT。既存 row の generation を増やす (§5.6.1 ステップ 1)。
    /// INDEX_VERSION=5 以降、原文 (`*_norm`) は Tantivy 側 (`*_text` STORED) で保持
    /// するので、ここでは管理メタのみ書く。
    pub fn mark_pending(
        &self,
        path: &str,
        favorite_id: Uuid,
        favorite_root: &Path,
        kind: IndexKind,
        mtime: i64,
        file_size: i64,
    ) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = now_epoch();
        conn.execute(
            "INSERT INTO files (
                path, favorite_id, favorite_root, kind, mtime, file_size,
                indexed_at, index_version, index_generation, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)
             ON CONFLICT(path) DO UPDATE SET
                favorite_id = excluded.favorite_id,
                favorite_root = excluded.favorite_root,
                kind = excluded.kind,
                mtime = excluded.mtime,
                file_size = excluded.file_size,
                indexed_at = excluded.indexed_at,
                index_version = excluded.index_version,
                index_generation = files.index_generation + 1,
                status = 1",
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
            ],
        )?;
        let gen_val: i64 = conn.query_row(
            "SELECT index_generation FROM files WHERE path = ?1",
            params![path],
            |r| r.get(0),
        )?;
        Ok(gen_val)
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
    /// Ctrl+G の post-filter は `filter_paths_status_ok` で status=0 のみ通すので、
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

    /// post-filter 用: 指定 path 群のうち `status=Ok` の path だけを返す (§5.6)。
    ///
    /// INDEX_VERSION=5 で原文を Tantivy 側に移したため、ここでは「Tantivy の検索
    /// snapshot に commit 済だが、二段整合性プロトコル上で `status=Pending`
    /// (ingest 進行中) / `Tombstone` (削除待ち) になっている doc」を弾くだけが目的。
    /// 部分インデックス `idx_files_status` は status != 0 を保持するので、
    /// この経路の `status = 0` フィルタは IN 句の path PK lookup でほぼ完結する。
    pub fn filter_paths_status_ok(&self, paths: &[String]) -> rusqlite::Result<Vec<String>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = sql_in_placeholders(paths.len());
        let sql = format!(
            "SELECT path FROM files WHERE status = 0 AND path IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_vec: Vec<&dyn rusqlite::ToSql> =
            paths.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::with_capacity(paths.len());
        for r in rows {
            out.push(r?);
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

    /// 起動時 reconciliation 用の最適化版: 指定お気に入り集合内で status != Ok の
    /// (path, favorite_id, status) を 1 クエリで返す。
    ///
    /// `list_not_ok_paths` をお気に入りごとにループすると `idx_files_fav_kind`
    /// が選ばれて status フィルタが post-filter 化し、お気に入り配下の **全行**
    /// (mIV では 65 万行で実測 1.1 秒) を読む羽目になる。これは部分インデックス
    /// `idx_files_status` (status != 0 の行だけを保持) を使えば 17ms で済む。
    /// `favorite_id IN (...)` で SQLite が自動的に部分インデックスを優先するため、
    /// 1 クエリにまとめて呼ぶ形にする。
    ///
    /// `favorite_ids` が空なら空配列を返す (status != 0 行が他お気に入りに残って
    /// いても、auto_index_metadata=true でない限り触らない既存の reconciliation 規約に従う)。
    pub fn list_not_ok_paths_for_favorites(
        &self,
        favorite_ids: &[Uuid],
    ) -> rusqlite::Result<Vec<(String, Uuid, FileStatus)>> {
        if favorite_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = sql_in_placeholders(favorite_ids.len());
        let sql = format!(
            "SELECT path, favorite_id, status FROM files \
             WHERE status != 0 AND favorite_id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_vec: Vec<String> = favorite_ids.iter().map(|id| id.to_string()).collect();
        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (p, fav_str, s) = row?;
            // favorite_id は uuid として保存されているはずだが、parse 失敗は skip して頑健に。
            let Ok(fav) = Uuid::parse_str(&fav_str) else {
                continue;
            };
            out.push((p, fav, FileStatus::from_i64(s)));
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

    /// favorite 配下の `status=Ok` 件数だけ高速に返す。
    ///
    /// `count_by_status` と違い 4 status 全部をスキャンせず、`idx_files_fav` で
    /// favorite_id を絞ってから status=0 だけカウントする。
    pub fn count_ok_for_favorite(&self, favorite_id: Uuid) -> rusqlite::Result<u64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM files WHERE favorite_id = ?1 AND status = 0",
            params![favorite_id.to_string()],
            |r| {
                let v: i64 = r.get(0)?;
                Ok(v as u64)
            },
        )
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
         indexed_at, index_version, index_generation, status"
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
    })
}

/// 既存の `files` テーブルが旧スキーマかを判定する (§19.8)。
///
/// 判定方針: `files` が存在し、かつ以下のいずれかに該当すれば旧版:
/// - 旧版の本文列 (`*_norm` 系) が残っている (INDEX_VERSION<=4 から 5 への移行)
/// - 旧 v1 の `all_text_norm` 列が残っている
/// - `index_version` 列の MIN 値が `INDEX_VERSION` より小さい
///
/// テーブルが存在しなければ `false` (新規作成なので rebuild 不要)。
fn needs_rebuild(conn: &Connection) -> rusqlite::Result<bool> {
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
    // 旧 *_norm 系の列がいずれかでも残っていれば旧スキーマ → rebuild。
    // INDEX_VERSION=5 で原文を Tantivy に移したため、これらは新スキーマでは存在しない。
    let mut stmt = conn.prepare("PRAGMA table_info(files)")?;
    let mut has_legacy_text_col = false;
    let rows = stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        Ok(name)
    })?;
    for r in rows {
        let name = r?;
        if matches!(
            name.as_str(),
            "all_text_norm"
                | "name_norm"
                | "exif_norm"
                | "xmp_tweet_norm"
                | "png_prompt_norm"
                | "pdf_meta_norm"
                | "tags_norm"
        ) {
            has_legacy_text_col = true;
            break;
        }
    }
    drop(stmt);
    if has_legacy_text_col {
        return Ok(true);
    }
    // index_version が古い行が残っているケース (将来的な v5 → v6 移行用)。
    let min_ver: Option<i64> = conn
        .query_row("SELECT MIN(index_version) FROM files", [], |row| {
            row.get::<_, Option<i64>>(0)
        })?;
    Ok(matches!(min_ver, Some(v) if v < INDEX_VERSION))
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    // INDEX_VERSION=5 以降、原文 (`*_norm` 列) は Tantivy 側 (`*_text` STORED) へ
    // 移したので、このテーブルは管理メタ + status のみを保持する。
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
            status            INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_files_fav       ON files(favorite_id);
         CREATE INDEX IF NOT EXISTS idx_files_fav_mtime ON files(favorite_id, mtime);
         CREATE INDEX IF NOT EXISTS idx_files_fav_kind  ON files(favorite_id, kind);
         CREATE INDEX IF NOT EXISTS idx_files_status    ON files(status) WHERE status != 0;",
    )?;
    Ok(())
}

/// `IN (?1,?2,…?N)` 用の placeholder 文字列を生成する。
/// rusqlite には配列バインドが無いので各 IN 句で個別に組み立てる必要がある。
/// お気に入り / path 一括取得など小さい N (< 数百) 専用。
fn sql_in_placeholders(n: usize) -> String {
    (0..n)
        .map(|i| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(",")
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
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tmp_db() -> (TempDir, FtsMetaDb) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        let db = FtsMetaDb::open_at(&db_path).unwrap();
        (dir, db)
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
        let gen1 = db
            .mark_pending("c:/photos/a.jpg", fav, &root, IndexKind::Image, 100, 2048)
            .unwrap();
        assert_eq!(gen1, 1, "初回 ingest の generation は 1");

        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Pending);
        assert_eq!(got.kind, IndexKind::Image);
        assert_eq!(got.index_generation, 1);
        assert_eq!(got.favorite_id, fav);

        db.mark_ok(&["c:/photos/a.jpg".to_string()]).unwrap();
        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Ok);

        // 同じ path で再 ingest → generation += 1
        let gen2 = db
            .mark_pending("c:/photos/a.jpg", fav, &root, IndexKind::Image, 200, 2100)
            .unwrap();
        assert_eq!(gen2, 2);
        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Pending);
    }

    #[test]
    fn tombstone_then_purge_cycle() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/p");
        db.mark_pending("c:/p/1.jpg", fav, &root, IndexKind::Image, 1, 10)
            .unwrap();
        db.mark_pending("c:/p/2.jpg", fav, &root, IndexKind::Image, 2, 20)
            .unwrap();
        db.mark_ok(&["c:/p/1.jpg".to_string(), "c:/p/2.jpg".to_string()])
            .unwrap();

        // tombstone にすると list_favorite_files (status=Ok のみ) から消える
        db.mark_tombstone(&["c:/p/1.jpg".to_string()]).unwrap();
        let ok_rows = db.list_favorite_files(fav).unwrap();
        assert_eq!(ok_rows.len(), 1);
        assert_eq!(ok_rows[0].0, "c:/p/2.jpg");

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
        db.mark_pending("c:/a/1.jpg", fav_a, &root_a, IndexKind::Image, 1, 1)
            .unwrap();
        db.mark_pending("c:/a/2.jpg", fav_a, &root_a, IndexKind::Image, 2, 2)
            .unwrap();
        db.mark_pending("c:/b/1.jpg", fav_b, &root_b, IndexKind::Image, 3, 3)
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
    fn list_not_ok_returns_pending_and_failed() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/x");
        db.mark_pending("c:/x/1.jpg", fav, &root, IndexKind::Image, 1, 1)
            .unwrap();
        db.mark_pending("c:/x/2.jpg", fav, &root, IndexKind::Image, 2, 2)
            .unwrap();
        db.mark_pending("c:/x/3.jpg", fav, &root, IndexKind::Image, 3, 3)
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
    fn list_not_ok_paths_for_favorites_filters_by_fav_and_status() {
        let (_tmp, db) = tmp_db();
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let fav_c = Uuid::new_v4();
        let root_a = PathBuf::from("C:/a");
        let root_b = PathBuf::from("C:/b");
        let root_c = PathBuf::from("C:/c");

        // fav_a: ok 1, pending 1, failed 1, tombstone 1
        db.mark_pending("c:/a/1.jpg", fav_a, &root_a, IndexKind::Image, 1, 1)
            .unwrap();
        db.mark_pending("c:/a/2.jpg", fav_a, &root_a, IndexKind::Image, 2, 2)
            .unwrap();
        db.mark_pending("c:/a/3.jpg", fav_a, &root_a, IndexKind::Image, 3, 3)
            .unwrap();
        db.mark_pending("c:/a/4.jpg", fav_a, &root_a, IndexKind::Image, 4, 4)
            .unwrap();
        db.mark_ok(&["c:/a/1.jpg".to_string()]).unwrap();
        db.mark_failed("c:/a/3.jpg").unwrap();
        db.mark_tombstone(&["c:/a/4.jpg".to_string()]).unwrap();

        db.mark_pending("c:/b/1.jpg", fav_b, &root_b, IndexKind::Image, 1, 1)
            .unwrap();
        db.mark_pending("c:/c/1.jpg", fav_c, &root_c, IndexKind::Image, 1, 1)
            .unwrap();

        let rows = db
            .list_not_ok_paths_for_favorites(&[fav_a, fav_b])
            .unwrap();
        assert_eq!(rows.len(), 4);

        for (path, fav, _) in &rows {
            assert!(*fav == fav_a || *fav == fav_b, "unexpected fav for {path}");
        }

        let empty_in: Vec<Uuid> = Vec::new();
        let empty_out = db.list_not_ok_paths_for_favorites(&empty_in).unwrap();
        assert!(empty_out.is_empty());

        let paths: Vec<_> = rows.iter().map(|(p, _, _)| p.as_str()).collect();
        assert!(!paths.contains(&"c:/a/1.jpg"));
    }

    #[test]
    fn count_by_status_groups() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/y");
        for i in 0..5 {
            db.mark_pending(&format!("c:/y/{}.jpg", i), fav, &root, IndexKind::Image, i, 1)
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
        // §19.8 マイグレーション回帰: 旧版の `*_norm` 列を持つ DB を開いたら自動で
        // drop → recreate する (INDEX_VERSION=5 で原文を Tantivy 側に移した移行)。
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");

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

        let db = FtsMetaDb::open_at(&db_path).unwrap();
        assert!(db.get("c:/old.jpg").unwrap().is_none(), "旧データは消えた");

        // 新しい mark_pending が通る (norms 引数なし)
        let fav = Uuid::new_v4();
        db.mark_pending(
            "c:/new.jpg",
            fav,
            std::path::Path::new("C:/"),
            IndexKind::Image,
            1,
            1,
        )
        .unwrap();
        let row = db.get("c:/new.jpg").unwrap().unwrap();
        assert_eq!(row.path, "c:/new.jpg");
        assert_eq!(row.status, FileStatus::Pending);
    }

    #[test]
    fn opening_v4_schema_with_norm_columns_triggers_rebuild() {
        // INDEX_VERSION=4 → 5 移行: per-source `*_norm` 列を持つ DB を開くと rebuild される。
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE files (
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
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO files VALUES ('c:/v4.jpg', ?1, 'C:/', 1, 1, 1, 1, 4, 1, 0, '', '', '', '', '', '')",
                params![Uuid::new_v4().to_string()],
            )
            .unwrap();
        }
        let db = FtsMetaDb::open_at(&db_path).unwrap();
        assert!(db.get("c:/v4.jpg").unwrap().is_none(), "v4 行は消えた");
        assert!(db.rebuilt_on_open(), "rebuilt フラグが true");
    }

    #[test]
    fn opening_fresh_db_does_not_trigger_rebuild() {
        let dir = TempDir::new().unwrap();
        let db = FtsMetaDb::open_at(&dir.path().join("fresh.db")).unwrap();
        assert!(db.list_not_ok().unwrap().is_empty());
        assert!(!db.rebuilt_on_open());
    }
}
