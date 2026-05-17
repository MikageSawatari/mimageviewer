//! 変換済みアーカイブ (7z / LZH → ZIP) のキャッシュ管理 (v0.7.0)。
//!
//! [`archive_converter`] が生成する無圧縮 ZIP を
//! `<data_dir>/archive_cache/<hash>/<basename>.zip` に保存し、
//! SQLite DB `<data_dir>/archive_cache.db` に元ファイルとの対応を記録する。
//!
//! # 検証方針
//!
//! `lookup` は「元ファイルの mtime + size が変わっていないこと」を判定基準とする。
//! 片方でも変わっていたらキャッシュは無効とみなして再変換する。
//!
//! # 削除管理
//!
//! サムネイルキャッシュと異なり 1 エントリあたり数百 MB 〜 GB オーダーになる。
//! ユーザーが容量を把握して手動で整理できるよう、
//! - 全エントリ一覧 (元ファイル存否を含む)
//! - 個別削除 / 元ファイル消失エントリの一括削除 / 全削除
//! を [`cache_manager`] ダイアログタブから操作できるようにする。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use crate::archive_converter::ArchiveFormat;

/// キャッシュルート (`<data_dir>/archive_cache`) を返す。呼び出し側で作成する必要はない
/// ([`ArchiveCacheDb::reserve_cache_zip_path`] が親ディレクトリを作る)。
pub fn cache_root() -> PathBuf {
    crate::data_dir::get().join("archive_cache")
}

/// DB ファイルのパス。
pub fn db_path() -> PathBuf {
    crate::data_dir::get().join("archive_cache.db")
}

// ──────────────────────────────────────────────────────────────────────
// パスハッシュ (キャッシュ ZIP の保存先を決定する)
// ──────────────────────────────────────────────────────────────────────

/// 元ファイルパスから変換済み ZIP の保存先を決定する。
/// mtime / size は含まない (元ファイルが更新されても同じ場所に上書きして冗長ファイルを残さない)。
fn path_hash(src: &Path) -> String {
    let normalized = crate::path_key::normalize(src);
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

/// 元ファイルから変換済み ZIP の絶対パスを決定する。
/// `<cache_root>/<hash前2文字>/<hash>/<basename>.zip`
fn cache_zip_path_for(src: &Path) -> PathBuf {
    let hash = path_hash(src);
    let basename = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("archive");
    cache_root()
        .join(&hash[..2])
        .join(&hash)
        .join(format!("{basename}.zip"))
}

// ──────────────────────────────────────────────────────────────────────
// エントリ型 (管理 UI 用)
// ──────────────────────────────────────────────────────────────────────

/// DB に記録されている変換済みアーカイブ 1 件分の情報。管理 UI で表示する。
#[derive(Debug, Clone)]
pub struct ArchiveCacheEntry {
    /// 元ファイルの絶対パス
    pub src_path: PathBuf,
    /// 元ファイルの mtime (UNIX 秒)
    pub src_mtime: i64,
    /// 元ファイルのバイトサイズ (記録時点)
    pub src_size: i64,
    /// 変換形式 (7z / LZH)
    pub format: ArchiveFormat,
    /// 変換済み ZIP の絶対パス
    pub cached_zip_path: PathBuf,
    /// 変換済み ZIP のバイトサイズ (記録時点)。ファイルが消えていたら 0。
    pub cached_zip_size: i64,
    /// 変換された日時 (UNIX 秒)
    pub converted_at: i64,
    /// 最後にこのキャッシュを使用した日時 (UNIX 秒)
    pub last_access_at: i64,
    /// 変換対象となった画像エントリ数
    pub image_count: i64,
    /// 元ファイルが現在もディスク上に存在するか
    pub src_exists: bool,
}

// ──────────────────────────────────────────────────────────────────────
// DB
// ──────────────────────────────────────────────────────────────────────

/// 変換済みアーカイブの対応表を保持する SQLite DB。
/// 内部 `Connection` は `Mutex` で保護される。
pub struct ArchiveCacheDb {
    conn: Mutex<Connection>,
    /// 変換 (`reserve_cache_zip_path` → 実際のファイル write → `record`) と
    /// 保守 (`delete_entry` / `clear_all`) を直列化するための独立 lock。
    ///
    /// `converted_at` は秒精度なので、snapshot 直後の再変換と DB 行を区別できず、
    /// `cached_zip_path` も src に対して決定的なので並行 worker が同じパスを再利用する。
    /// これらを避けるため:
    /// - 変換 worker は [`Self::begin_convert`] で guard を取り、write + record 完了まで保持する。
    /// - `delete_entry` / `clear_all` は内部でこの lock を取ってから snapshot + FS 削除 +
    ///   DB DELETE を行う。
    ///
    /// 保守中は新しい変換が待たされるが、ユーザーが明示的に全削除 / 個別削除を
    /// 指示したタイミングでしか起きないので許容する。変換中に保守が待たされる
    /// ケースは通常秒オーダーで収束する。
    convert_lock: Mutex<()>,
}

impl ArchiveCacheDb {
    /// DB を開く (なければ作成)。`<data_dir>` 配下に書き込む。
    pub fn open() -> rusqlite::Result<Self> {
        let path = db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            convert_lock: Mutex::new(()),
        })
    }

    /// 変換 worker は write + record を始める前にこの guard を取り、完了まで保持する。
    /// maintenance (`delete_entry` / `clear_all`) は同じ lock を取るため、変換中は待つ。
    /// 呼び出し側で poisoned 時は内側を取り出す — key は「記録欠落」ではなく「直列化」。
    pub fn begin_convert(&self) -> std::sync::MutexGuard<'_, ()> {
        self.convert_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 有効なキャッシュがあれば ZIP パスを返し、`last_access_at` を更新する。
    ///
    /// 「有効」の条件:
    /// - DB にエントリがある
    /// - 記録されている mtime / size が現在の元ファイルと一致
    /// - 変換済み ZIP ファイルがディスク上に存在する
    ///
    /// いずれかが満たせない場合は `None` を返し、無効エントリは DB から掃除する。
    ///
    /// DB mutex の保持区間は (1) SELECT と (2) UPDATE/DELETE の 2 回だけに切り、
    /// `remove_file` / `exists()` は lock 外で実行する。UI スレッドの fullscreen / サムネ経由で
    /// 頻繁に呼ばれる経路なので、1 エントリの FS syscall のたびに他の DB 操作が詰まらない
    /// ようにするのが狙い。
    pub fn lookup(&self, src_path: &Path, src_mtime: i64, src_size: i64) -> Option<PathBuf> {
        let key = crate::path_key::normalize(src_path);
        // Phase 1: SELECT (短時間 lock)
        let row: Option<(i64, i64, String)> = {
            let conn = self.conn.lock().ok()?;
            conn.query_row(
                "SELECT src_mtime, src_size, cached_zip_path FROM converted_archives \
                 WHERE src_path_key = ?1",
                params![&key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok()
        };
        let (m, s, cached) = row?;

        // Phase 2: FS I/O (lock 外)
        let mismatch = m != src_mtime || s != src_size;
        let zip_path = PathBuf::from(&cached);
        let zip_exists = !mismatch && zip_path.exists();
        if mismatch {
            let _ = std::fs::remove_file(&cached);
        }

        // Phase 3: UPDATE / DELETE (短時間 lock)
        //
        // Phase 1 の SELECT から Phase 3 の DELETE までの間に、`record()` が同じ src に対して
        // INSERT OR REPLACE で新しい変換結果を書き込む可能性がある (ユーザが並行で同じ
        // アーカイブを開いて変換し直したケース)。単純に `src_path_key = ?` で DELETE すると
        // その新エントリまで巻き込んでしまうので、WHERE に Phase 1 で読んだ `src_mtime` +
        // `src_size` を含めて「自分が見た時点の古い行」にだけ DELETE がヒットするようにする。
        // 同じ条件で UPDATE もしておき、valid だったエントリを別のワーカーが同時に差し替えて
        // いた場合はこちらの last_access_at 更新を素通しにする (新エントリの last_access_at を
        // 古く上書きしてしまうのを避ける)。
        let conn = self.conn.lock().ok()?;
        if mismatch || !zip_exists {
            let _ = conn.execute(
                "DELETE FROM converted_archives \
                 WHERE src_path_key = ?1 AND src_mtime = ?2 AND src_size = ?3",
                params![&key, m, s],
            );
            return None;
        }
        let now = now_secs();
        let _ = conn.execute(
            "UPDATE converted_archives SET last_access_at = ?1 \
             WHERE src_path_key = ?2 AND src_mtime = ?3 AND src_size = ?4",
            params![now, &key, m, s],
        );
        Some(zip_path)
    }

    /// 変換完了時に 1 行 upsert する。
    pub fn record(
        &self,
        src_path: &Path,
        src_mtime: i64,
        src_size: i64,
        format: ArchiveFormat,
        cached_zip_path: &Path,
        cached_zip_size: i64,
        image_count: u32,
    ) -> rusqlite::Result<()> {
        let key = crate::path_key::normalize(src_path);
        let src_str = src_path.to_string_lossy().to_string();
        let cached_str = cached_zip_path.to_string_lossy().to_string();
        let format_str = format_to_db(format);
        let now = now_secs();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO converted_archives \
             (src_path_key, src_path, src_mtime, src_size, format, \
              cached_zip_path, cached_zip_size, converted_at, last_access_at, image_count) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
            params![
                &key,
                src_str,
                src_mtime,
                src_size,
                format_str,
                cached_str,
                cached_zip_size,
                now,
                image_count as i64,
            ],
        )?;
        Ok(())
    }

    /// 全エントリを `ArchiveCacheEntry` のリストとして返す (最終アクセス降順)。
    ///
    /// DB 読み出しだけ mutex 保持し、各 src_path の `exists()` チェック (per-row FS syscall)
    /// は lock を落としてから実行する。件数が多いと数十〜数百 ms かかるので、その間に UI が
    /// 別の DB 操作で mutex 待ちにならないようにする。
    pub fn list_all(&self) -> rusqlite::Result<Vec<ArchiveCacheEntry>> {
        let raw: Vec<(String, i64, i64, String, String, i64, i64, i64, i64)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT src_path, src_mtime, src_size, format, \
                        cached_zip_path, cached_zip_size, converted_at, last_access_at, image_count \
                 FROM converted_archives \
                 ORDER BY last_access_at DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            })?;
            rows.flatten().collect()
        };
        let mut out = Vec::with_capacity(raw.len());
        for (
            src_str,
            src_mtime,
            src_size,
            format_str,
            cached_str,
            cached_zip_size,
            converted_at,
            last_access_at,
            image_count,
        ) in raw
        {
            let src_path = PathBuf::from(&src_str);
            let cached_zip_path = PathBuf::from(&cached_str);
            let Some(format) = format_from_db(&format_str) else {
                continue;
            };
            let src_exists = src_path.exists();
            out.push(ArchiveCacheEntry {
                src_path,
                src_mtime,
                src_size,
                format,
                cached_zip_path,
                cached_zip_size,
                converted_at,
                last_access_at,
                image_count,
                src_exists,
            });
        }
        Ok(out)
    }

    /// 指定した元ファイルに対応するキャッシュを削除する (DB 行 + ZIP ファイル + 親ディレクトリ)。
    ///
    /// DB mutex は lookup と DELETE の短時間だけ保持し、ファイル削除は lock 外で実行する。
    /// こうしないと 1 エントリの remove_file + 親ディレクトリ掃除を保持中に UI スレッドの
    /// lookup / total_size が待たされる。
    ///
    /// 並行変換 worker との race は [`Self::convert_lock`] で排他化する。秒精度の
    /// `converted_at` では snapshot 直後の再変換を区別できず、`cached_zip_path` は src に
    /// 対して決定的なので path も共有する — mtime ヒューリスティックでは防ぎきれないため、
    /// convert_lock で直列化して変換中は保守を待たせる。
    pub fn delete_entry(&self, src_path: &Path) -> rusqlite::Result<()> {
        let _convert_guard = self.begin_convert();
        let key = crate::path_key::normalize(src_path);
        let row: Option<String> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT cached_zip_path FROM converted_archives WHERE src_path_key = ?1",
                params![&key],
                |r| r.get(0),
            )
            .ok()
        };
        let Some(cached) = row else {
            return Ok(());
        };
        remove_cache_file_and_dirs(Path::new(&cached));
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM converted_archives WHERE src_path_key = ?1",
            params![&key],
        )?;
        Ok(())
    }

    /// 元ファイルが既に存在しないエントリを一括削除する。
    /// 戻り値は削除したエントリ数。
    pub fn delete_missing_originals(&self) -> rusqlite::Result<usize> {
        let entries = self.list_all()?;
        let mut removed = 0;
        for e in entries.iter().filter(|e| !e.src_exists) {
            if self.delete_entry(&e.src_path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// snapshot 時点で DB に登録されていたエントリを削除する。
    /// 戻り値は削除したエントリ数。
    ///
    /// 並行して走っている変換ワーカーが `record()` した新 entry を巻き込まないよう、
    /// 最初に取った (src_path_key, cached_zip_path) のスナップショットだけを対象にする。
    /// FS I/O (個別 `remove_file` + 空になった親ディレクトリの `remove_dir`) は lock 外、
    /// DELETE は snapshot 対象の key を 1 件ずつ DELETE するため、snapshot 後に
    /// 追加された新エントリは DB にもファイルにも残る。
    /// 以前は `remove_dir_all(cache_root)` + 無条件 `DELETE FROM converted_archives` で
    /// 丸ごと掃除していたが、タイミング次第で並行変換完了の成果物を吹き飛ばす競合があった。
    pub fn clear_all(&self) -> rusqlite::Result<usize> {
        // 変換 worker と排他化。begin_convert 保持中は新規 convert/record がブロックされる。
        // 全削除ワーカー自体が UI とは別スレッドなので、UI のレスポンスは影響しない。
        let _convert_guard = self.begin_convert();

        let snapshot: Vec<(String, String)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt =
                conn.prepare("SELECT src_path_key, cached_zip_path FROM converted_archives")?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .flatten()
                .collect()
        };
        // convert_lock 保持中は並行 record() が走らないので、snapshot 〜 FS 削除 〜 DB DELETE の
        // 範囲内で「別 worker が同じパスを書き戻す」race は発生しない。
        for (_, p) in &snapshot {
            remove_cache_file_and_dirs(Path::new(p));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut deleted = 0usize;
        {
            let mut stmt = tx.prepare("DELETE FROM converted_archives WHERE src_path_key = ?1")?;
            for (key, _) in &snapshot {
                deleted += stmt.execute(params![key])?;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// 合計キャッシュ容量 (バイト) を返す。
    pub fn total_size(&self) -> rusqlite::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(cached_zip_size), 0) FROM converted_archives",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(total.max(0) as u64)
    }

    /// 変換完了時に使う予定の出力パスを返す (親ディレクトリも作成する)。
    /// ファイル自体は作成しない。
    pub fn reserve_cache_zip_path(&self, src: &Path) -> std::io::Result<PathBuf> {
        let path = cache_zip_path_for(src);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(path)
    }
}

// ──────────────────────────────────────────────────────────────────────
// 内部ヘルパー
// ──────────────────────────────────────────────────────────────────────

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS converted_archives (\
            src_path_key     TEXT PRIMARY KEY, \
            src_path         TEXT NOT NULL, \
            src_mtime        INTEGER NOT NULL, \
            src_size         INTEGER NOT NULL, \
            format           TEXT NOT NULL, \
            cached_zip_path  TEXT NOT NULL, \
            cached_zip_size  INTEGER NOT NULL, \
            converted_at     INTEGER NOT NULL, \
            last_access_at   INTEGER NOT NULL, \
            image_count      INTEGER NOT NULL\
         );",
    )
}

fn format_to_db(f: ArchiveFormat) -> &'static str {
    match f {
        ArchiveFormat::SevenZ => "7z",
        ArchiveFormat::Lzh => "lzh",
    }
}

fn format_from_db(s: &str) -> Option<ArchiveFormat> {
    match s {
        "7z" => Some(ArchiveFormat::SevenZ),
        "lzh" | "lha" => Some(ArchiveFormat::Lzh),
        _ => None,
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// キャッシュ ZIP ファイルを削除し、空になった親 (`<hash>/`) と祖父 (`<hash前2文字>/`)
/// ディレクトリも削除する。失敗は無視 (キャッシュなので restartable)。
fn remove_cache_file_and_dirs(zip_path: &Path) {
    let _ = std::fs::remove_file(zip_path);
    let root = cache_root();
    if let Some(parent) = zip_path.parent() {
        if parent.starts_with(&root) {
            let _ = std::fs::remove_dir(parent);
            if let Some(grand) = parent.parent() {
                if grand != root && grand.starts_with(&root) {
                    let _ = std::fs::remove_dir(grand);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_data_dir() -> TempDir {
        let tmp = TempDir::new().unwrap();
        // テストごとに DATA_DIR は上書きできないので、OnceLock の初回 set 狙い。
        // 既に設定されていれば current DATA_DIR を尊重する。
        crate::data_dir::DATA_DIR.set(tmp.path().to_path_buf()).ok();
        tmp
    }

    #[test]
    fn schema_and_roundtrip() {
        let _guard = fresh_data_dir();
        let db = ArchiveCacheDb::open().unwrap();
        assert!(db.list_all().unwrap().is_empty());
        assert_eq!(db.total_size().unwrap(), 0);
    }

    #[test]
    fn path_hash_stable() {
        let a = path_hash(Path::new(r"C:\foo\bar.7z"));
        let b = path_hash(Path::new(r"C:\foo\bar.7z"));
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn cache_zip_path_uses_basename() {
        // 2026-05-17: `cache_zip_path_for` 内部の `cache_root()` が `data_dir::get()` を
        // 呼ぶので、`cfg(test)` の panic ガード回避のため override を立てる。
        let _g = crate::data_dir::TestDataDirGuard::new();
        let p = cache_zip_path_for(Path::new(r"C:\archives\manga_vol01.7z"));
        assert!(p.to_string_lossy().ends_with("manga_vol01.zip"));
    }
}
