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

use std::collections::HashMap;
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
/// - 6: status を Ok / Failed の 2 値に縮小。Pending / Tombstone を廃止し、検索
///      post-filter の SQLite SELECT も削除。
pub const INDEX_VERSION: i64 = 6;

/// 後始末 (VACUUM 等) を要求するスキーマ世代。`PRAGMA application_id` に書き込み、
/// 既に最新なら再実行しない。INDEX_VERSION とは別管理で、データ移行を伴わない
/// 後始末だけバンプしたいケースに対応する。
///
/// ## bump 履歴
/// - 1: INDEX_VERSION=5 移行で `*_norm` 列を撤去した後の VACUUM (空きページ解放)
const HOUSEKEEPING_VERSION: i32 = 1;

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

/// `from_i64` は不正値や旧 schema の Pending(1) / Tombstone(3) を `Failed` に倒す
/// (= reconciliation 経路で再 ingest 候補にする)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum FileStatus {
    Ok = 0,
    Failed = 2,
}

impl FileStatus {
    fn from_i64(v: i64) -> Self {
        match v {
            0 => Self::Ok,
            _ => Self::Failed,
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
        // v5 → v6 はスキーマ列構造が互換 (status を 4 値 → 2 値に運用変更しただけ) なので
        // テーブル drop しない。それ以外の未知 version は needs_rebuild が判断する。
        let rebuild_needed = match user_version {
            INDEX_VERSION => false,
            5 => false,
            _ => needs_rebuild(&conn)?,
        };
        if rebuild_needed {
            crate::logger::log(format!(
                "fts_meta: detected old schema (index_version < {INDEX_VERSION}) — dropping `files` table for rebuild"
            ));
            conn.execute_batch("DROP TABLE IF EXISTS files;")?;
        }
        init_schema(&conn)?;

        // v5 → v6 データマイグレーション: status を Ok(0) と Failed(2) の 2 値に正規化。
        // Pending(1) と Tombstone(3) はどちらも Failed に倒す。
        // - Pending: クラッシュ等で「ingest 進行中」だったもの → 再 ingest 候補に。
        // - Tombstone: 削除予定だったもの → reconciliation で Tantivy delete + 物理削除
        //   される。物理 DELETE してしまうと Tantivy 側に対応 doc が残った場合に
        //   検索結果に出続ける regression になるので、Failed 経由で reconciliation に
        //   委ねる。
        // 1 トランザクションでまとめ、user_version の bump も同じ tx に含める。
        if user_version != INDEX_VERSION {
            let tx = conn.unchecked_transaction()?;
            if user_version != 0 && user_version < INDEX_VERSION {
                let pending = tx.execute("UPDATE files SET status = 2 WHERE status = 1", [])?;
                let tombstones = tx.execute("UPDATE files SET status = 2 WHERE status = 3", [])?;
                if pending > 0 || tombstones > 0 {
                    crate::logger::log(format!(
                        "fts_meta: v{user_version}→v{INDEX_VERSION} migration: \
                         pending→failed={pending} rows, tombstone→failed={tombstones} rows"
                    ));
                }
            }
            tx.execute_batch(&format!("PRAGMA user_version = {INDEX_VERSION};"))?;
            tx.commit()?;
        }
        // 後始末 (VACUUM) は起動経路から外し、`maybe_run_housekeeping_async` で起動後に
        // バックグラウンド実行する。数 GB で数分かかりうるので起動 overlay を止めない方針。
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

    /// 起動後に呼ぶ housekeeping。`application_id != HOUSEKEEPING_VERSION` の場合のみ
    /// VACUUM を 1 回走らせる (数 GB で数分かかりうる)。失敗時は marker を書かないので
    /// 次回起動で再試行される。Mutex を取って実行するので、ingest との同時実行は
    /// 自動的に直列化される (write lock 取得待ち)。
    pub fn run_housekeeping_if_needed(&self, db_path: &Path) {
        let conn = self.conn.lock().unwrap();
        let app_id: i32 = conn
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .unwrap_or(0);
        if app_id == HOUSEKEEPING_VERSION {
            return;
        }
        let before = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
        crate::logger::log(format!(
            "fts_meta: running housekeeping VACUUM (housekeeping {HOUSEKEEPING_VERSION}, \
             file size {before} bytes)"
        ));
        let t = std::time::Instant::now();
        match conn.execute_batch("VACUUM;") {
            Ok(()) => {
                let elapsed = t.elapsed();
                let after = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
                if let Err(e) = conn.execute_batch(&format!(
                    "PRAGMA application_id = {HOUSEKEEPING_VERSION};"
                )) {
                    crate::logger::log(format!(
                        "fts_meta: housekeeping marker bump failed (will retry): {e}"
                    ));
                    return;
                }
                crate::logger::log(format!(
                    "fts_meta: VACUUM done in {:?} ({} → {} bytes, reclaimed {} bytes)",
                    elapsed,
                    before,
                    after,
                    before.saturating_sub(after)
                ));
            }
            Err(e) => {
                crate::logger::log(format!(
                    "fts_meta: VACUUM failed (non-fatal, will retry next launch): {e}"
                ));
            }
        }
    }

    /// status=Ok で UPSERT。既存 row の generation を増やす。Tantivy commit と
    /// 同じバッチで呼び、commit が失敗した場合の検出は起動時 reconciliation の
    /// 3-way diff (FS / Tantivy / SQLite) に任せる。
    pub fn upsert_meta_ok(
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
                status = 0",
            params![
                path,
                favorite_id.to_string(),
                favorite_root.to_string_lossy().into_owned(),
                kind.to_i64(),
                mtime,
                file_size,
                now,
                INDEX_VERSION,
                FileStatus::Ok as i64,
            ],
        )?;
        let gen_val: i64 = conn.query_row(
            "SELECT index_generation FROM files WHERE path = ?1",
            params![path],
            |r| r.get(0),
        )?;
        Ok(gen_val)
    }

    /// ingest 失敗を記録。次回再試行 (retry 抑制は上位レイヤーで管理)。
    pub fn mark_failed(&self, path: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE files SET status = ?1 WHERE path = ?2",
            params![FileStatus::Failed as i64, path],
        )?;
        Ok(())
    }

    /// お気に入り配下の全行を物理削除する (favorite の「メタ」チェックを OFF にした時)。
    /// 返り値は削除した行数。Tantivy 側の delete は呼び出し側の責務。
    pub fn delete_all_for_favorite(&self, favorite_id: Uuid) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            "DELETE FROM files WHERE favorite_id = ?1",
            params![favorite_id.to_string()],
        )?;
        Ok(deleted)
    }

    /// 指定 path 群の行を物理削除する。Tantivy 側 delete 完了後の cleanup として呼ぶ。
    pub fn delete_paths(&self, paths: &[String]) -> rusqlite::Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = sql_in_placeholders(paths.len());
        let sql = format!("DELETE FROM files WHERE path IN ({placeholders})");
        let params_vec: Vec<&dyn rusqlite::ToSql> =
            paths.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let deleted = conn.execute(&sql, rusqlite::params_from_iter(params_vec))?;
        Ok(deleted)
    }

    /// 指定 favorite_id 配下の全 path を返す (status 不問)。
    /// `purge_favorite_metadata` で Tantivy 側にも delete_term を投げるための列挙用。
    pub fn list_all_paths_for_favorite(
        &self,
        favorite_id: Uuid,
    ) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT path FROM files WHERE favorite_id = ?1")?;
        let rows = stmt.query_map(params![favorite_id.to_string()], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
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

    /// 全 favorite の `status=Ok` 件数を 1 クエリで返す (UI 表示の一括集計用)。
    ///
    /// 個別 `count_ok_for_favorite` を N 回呼ぶと毎回 connection mutex を取り
    /// background writer (ingest_worker / mark_ok) と競合するため、まとめて 1 回で
    /// 取得する。返却値は `favorite_id` (UUID parse 後) → 件数。Ok 件数 0 の
    /// favorite は含まれない。
    pub fn count_ok_grouped_by_favorite(&self) -> rusqlite::Result<HashMap<Uuid, u64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT favorite_id, COUNT(*) FROM files WHERE status = 0 GROUP BY favorite_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out: HashMap<Uuid, u64> = HashMap::new();
        for r in rows {
            let (uuid_str, c) = r?;
            if let Ok(id) = Uuid::parse_str(&uuid_str) {
                out.insert(id, c as u64);
            }
        }
        Ok(out)
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
                FileStatus::Failed => counts.failed = c as usize,
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
    pub failed: usize,
}

impl StatusCounts {
    pub fn total(self) -> usize {
        self.ok + self.failed
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

    /// 初回 open で `application_id` が `HOUSEKEEPING_VERSION` に設定され、
    /// 2 回目以降の open では VACUUM がスキップされる。
    #[test]
    fn housekeeping_marker_set_after_explicit_run() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        let db = FtsMetaDb::open_at(&db_path).unwrap();
        // 起動経路で自動実行されないことを確認
        {
            let conn = Connection::open(&db_path).unwrap();
            let app_id: i32 = conn
                .query_row("PRAGMA application_id", [], |r| r.get(0))
                .unwrap_or(0);
            assert_eq!(app_id, 0, "open_at だけでは housekeeping は走らない");
        }
        // 明示的に走らせると marker が立つ
        db.run_housekeeping_if_needed(&db_path);
        drop(db);
        let conn = Connection::open(&db_path).unwrap();
        let app_id: i32 = conn
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .unwrap();
        assert_eq!(app_id, HOUSEKEEPING_VERSION);
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
    fn upsert_meta_ok_writes_status_ok() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/photos");
        let gen1 = db
            .upsert_meta_ok("c:/photos/a.jpg", fav, &root, IndexKind::Image, 100, 2048)
            .unwrap();
        assert_eq!(gen1, 1, "初回 ingest の generation は 1");

        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Ok);
        assert_eq!(got.kind, IndexKind::Image);
        assert_eq!(got.index_generation, 1);
        assert_eq!(got.favorite_id, fav);

        let gen2 = db
            .upsert_meta_ok("c:/photos/a.jpg", fav, &root, IndexKind::Image, 200, 2100)
            .unwrap();
        assert_eq!(gen2, 2, "再 ingest で generation が増える");
        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Ok);
    }

    #[test]
    fn delete_paths_physically_removes_rows() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/p");
        db.upsert_meta_ok("c:/p/1.jpg", fav, &root, IndexKind::Image, 1, 10)
            .unwrap();
        db.upsert_meta_ok("c:/p/2.jpg", fav, &root, IndexKind::Image, 2, 20)
            .unwrap();

        let deleted = db.delete_paths(&["c:/p/1.jpg".to_string()]).unwrap();
        assert_eq!(deleted, 1);
        assert!(db.get("c:/p/1.jpg").unwrap().is_none());
        assert!(db.get("c:/p/2.jpg").unwrap().is_some());
    }

    #[test]
    fn delete_all_for_favorite_clears_rows() {
        let (_tmp, db) = tmp_db();
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let root_a = PathBuf::from("C:/a");
        let root_b = PathBuf::from("C:/b");
        db.upsert_meta_ok("c:/a/1.jpg", fav_a, &root_a, IndexKind::Image, 1, 1)
            .unwrap();
        db.upsert_meta_ok("c:/a/2.jpg", fav_a, &root_a, IndexKind::Image, 2, 2)
            .unwrap();
        db.upsert_meta_ok("c:/b/1.jpg", fav_b, &root_b, IndexKind::Image, 3, 3)
            .unwrap();

        let removed = db.delete_all_for_favorite(fav_a).unwrap();
        assert_eq!(removed, 2);
        assert!(db.list_favorite_files(fav_a).unwrap().is_empty());
        assert_eq!(db.list_favorite_files(fav_b).unwrap().len(), 1);
    }

    #[test]
    fn list_favorite_files_returns_only_ok() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/a");
        db.upsert_meta_ok("c:/a/1.jpg", fav, &root, IndexKind::Image, 1, 1)
            .unwrap();
        db.upsert_meta_ok("c:/a/2.jpg", fav, &root, IndexKind::Image, 2, 2)
            .unwrap();
        db.mark_failed("c:/a/2.jpg").unwrap();

        let ok = db.list_favorite_files(fav).unwrap();
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].0, "c:/a/1.jpg");
    }

    #[test]
    fn list_not_ok_returns_only_failed() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/x");
        db.upsert_meta_ok("c:/x/1.jpg", fav, &root, IndexKind::Image, 1, 1)
            .unwrap();
        db.upsert_meta_ok("c:/x/2.jpg", fav, &root, IndexKind::Image, 2, 2)
            .unwrap();
        db.mark_failed("c:/x/2.jpg").unwrap();

        let not_ok = db.list_not_ok().unwrap();
        assert_eq!(not_ok.len(), 1);
        assert_eq!(not_ok[0].status, FileStatus::Failed);
        assert_eq!(not_ok[0].path, "c:/x/2.jpg");
    }

    #[test]
    fn count_ok_grouped_by_favorite_returns_only_ok() {
        let (_tmp, db) = tmp_db();
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let root_a = PathBuf::from("C:/a");
        let root_b = PathBuf::from("C:/b");
        for i in 1..=3 {
            db.upsert_meta_ok(
                &format!("c:/a/{i}.jpg"),
                fav_a,
                &root_a,
                IndexKind::Image,
                i,
                i,
            )
            .unwrap();
        }
        db.mark_failed("c:/a/3.jpg").unwrap();
        db.upsert_meta_ok("c:/b/1.jpg", fav_b, &root_b, IndexKind::Image, 1, 1)
            .unwrap();
        let fav_c = Uuid::new_v4();
        db.upsert_meta_ok("c:/c/1.jpg", fav_c, &PathBuf::from("C:/c"), IndexKind::Image, 1, 1)
            .unwrap();
        db.mark_failed("c:/c/1.jpg").unwrap();

        let counts = db.count_ok_grouped_by_favorite().unwrap();
        assert_eq!(counts.get(&fav_a), Some(&2));
        assert_eq!(counts.get(&fav_b), Some(&1));
        assert!(!counts.contains_key(&fav_c), "fav_c は ok 0 件なのでキー出ない");
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

        db.upsert_meta_ok("c:/a/1.jpg", fav_a, &root_a, IndexKind::Image, 1, 1)
            .unwrap();
        db.upsert_meta_ok("c:/a/2.jpg", fav_a, &root_a, IndexKind::Image, 2, 2)
            .unwrap();
        db.mark_failed("c:/a/2.jpg").unwrap();
        db.upsert_meta_ok("c:/b/1.jpg", fav_b, &root_b, IndexKind::Image, 1, 1)
            .unwrap();
        db.mark_failed("c:/b/1.jpg").unwrap();
        db.upsert_meta_ok("c:/c/1.jpg", fav_c, &root_c, IndexKind::Image, 1, 1)
            .unwrap();
        db.mark_failed("c:/c/1.jpg").unwrap();

        let rows = db
            .list_not_ok_paths_for_favorites(&[fav_a, fav_b])
            .unwrap();
        assert_eq!(rows.len(), 2);
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
            db.upsert_meta_ok(&format!("c:/y/{}.jpg", i), fav, &root, IndexKind::Image, i, 1)
                .unwrap();
        }
        db.mark_failed("c:/y/2.jpg").unwrap();
        db.mark_failed("c:/y/3.jpg").unwrap();

        let c = db.count_by_status(fav).unwrap();
        assert_eq!(c.ok, 3);
        assert_eq!(c.failed, 2);
        assert_eq!(c.total(), 5);
        assert_eq!(c.indexed(), 3);
    }

    #[test]
    fn migration_v5_to_v6_collapses_pending_and_drops_tombstones() {
        // 旧 v5 スキーマで status=Pending(1) と Tombstone(3) を含むデータを作り、
        // INDEX_VERSION=6 で開き直したときに pending→failed, tombstone→DELETE される。
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            // v5 スキーマ (= 現スキーマと列構造同じ)。手動で作って status を旧値で入れる。
            conn.execute_batch(
                "CREATE TABLE files (
                    path              TEXT PRIMARY KEY,
                    favorite_id       TEXT NOT NULL,
                    favorite_root     TEXT NOT NULL,
                    kind              INTEGER NOT NULL,
                    mtime             INTEGER NOT NULL,
                    file_size         INTEGER NOT NULL,
                    indexed_at        INTEGER NOT NULL,
                    index_version     INTEGER NOT NULL,
                    index_generation  INTEGER NOT NULL,
                    status            INTEGER NOT NULL
                 );
                 PRAGMA user_version = 5;",
            )
            .unwrap();
            let fav = Uuid::new_v4().to_string();
            for (path, status) in [
                ("c:/a/ok.jpg", 0),
                ("c:/a/pending.jpg", 1),
                ("c:/a/failed.jpg", 2),
                ("c:/a/tomb.jpg", 3),
            ] {
                conn.execute(
                    "INSERT INTO files VALUES (?1, ?2, 'C:/a', 1, 0, 0, 0, 5, 1, ?3)",
                    params![path, fav, status],
                )
                .unwrap();
            }
        }
        let db = FtsMetaDb::open_at(&db_path).unwrap();
        assert!(db.get("c:/a/ok.jpg").unwrap().is_some());
        assert_eq!(
            db.get("c:/a/pending.jpg").unwrap().unwrap().status,
            FileStatus::Failed,
            "pending は failed に降格"
        );
        assert!(db.get("c:/a/failed.jpg").unwrap().is_some());
        assert_eq!(
            db.get("c:/a/tomb.jpg").unwrap().unwrap().status,
            FileStatus::Failed,
            "tombstone は failed に降格 (Tantivy delete を reconciliation に委ねる)"
        );
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

        // 新しい upsert_meta_ok が通る
        let fav = Uuid::new_v4();
        db.upsert_meta_ok(
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
        assert_eq!(row.status, FileStatus::Ok);
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
