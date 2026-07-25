//! 変換済みアーカイブ (RAR / 7z / LZH / 非 ZIP 入れ子入り ZIP → ZIP) のキャッシュ管理。
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

/// `path` が変換キャッシュ ZIP の置き場 (cache_root 配下) にあるか。
/// キャッシュ ZIP 自身を開いたときに「入れ子アーカイブ検出 → 再変換提案」の
/// ループに入らないためのガードに使う (v1.3.0)。
pub fn is_under_cache_root(path: &Path) -> bool {
    path.starts_with(cache_root())
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
    cache_zip_path_for_data_dir(&crate::data_dir::get(), src)
}

/// ポータブルメタ情報のimportなど、通常profileとは別のdata directoryに対しても
/// 変換cacheの安定keyを再現するための純粋なpath計算。
pub(crate) fn cache_zip_path_for_data_dir(data_dir: &Path, src: &Path) -> PathBuf {
    let hash = path_hash(src);
    let basename = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("archive");
    data_dir
        .join("archive_cache")
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
    /// 変換形式 (RAR / 7z / LZH / ZIP)。将来版または旧版由来で不明な DB 値なら None。
    pub format: Option<ArchiveFormat>,
    /// DB に保存されていた format 文字列。不明形式でも管理 UI で表示・削除できるよう保持する。
    pub format_raw: String,
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
    /// 変換時にパスワードが必要だったか。
    /// 管理 UI の表示用メタ情報であり、キャッシュ ZIP 自体は暗号化されない。
    pub password_required: bool,
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

    /// [`Self::lookup`] の読み取り専用版。`last_access_at` を更新せず、無効エントリの
    /// 掃除もしない (SELECT + `exists()` のみ、書き込みトランザクションなし)。
    ///
    /// 用途: フォルダ一覧の `ConvertibleArchive` サムネ用 cache path 解決 worker。
    /// フォルダを**表示しただけ**で中の全アーカイブに UPDATE を発行しない / LRU prune の
    /// 「最終アクセス」を閲覧していないアーカイブで汚さないため。実際に開く経路
    /// (`try_archive_cache_lookup`) は従来どおり `lookup` を使い、access 時刻を更新する。
    pub fn peek(&self, src_path: &Path, src_mtime: i64, src_size: i64) -> Option<PathBuf> {
        let key = crate::path_key::normalize(src_path);
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
        if m != src_mtime || s != src_size {
            return None;
        }
        let zip_path = PathBuf::from(cached);
        zip_path.exists().then_some(zip_path)
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
        password_required: bool,
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
              cached_zip_path, cached_zip_size, converted_at, last_access_at, image_count, password_required) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10)",
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
                if password_required { 1i64 } else { 0i64 },
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
        let raw: Vec<(String, i64, i64, String, String, i64, i64, i64, i64, i64)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT src_path, src_mtime, src_size, format, \
                        cached_zip_path, cached_zip_size, converted_at, last_access_at, image_count, password_required \
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
                    r.get::<_, i64>(9)?,
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
            password_required,
        ) in raw
        {
            let src_path = PathBuf::from(&src_str);
            let cached_zip_path = PathBuf::from(&cached_str);
            let format = format_from_db(&format_str);
            let src_exists = src_path.exists();
            out.push(ArchiveCacheEntry {
                src_path,
                src_mtime,
                src_size,
                format,
                format_raw: format_str,
                cached_zip_path,
                cached_zip_size,
                converted_at,
                last_access_at,
                image_count,
                password_required: password_required != 0,
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

    /// 合計容量が `max_bytes` 以内になるまで、最終アクセスが古いキャッシュから削除する。
    ///
    /// `max_bytes == 0` は無制限。`protected_src_path` は直近で作成したキャッシュで、
    /// 単体で上限を超えていても削除しない。呼び出し側は変換完了直後に
    /// [`Self::begin_convert`] の guard を保持したまま呼ぶこと。これにより、変換完了直後の
    /// 自動掃除と明示的な管理操作 (`delete_entry` / `clear_all`) が同時に同じファイルを
    /// 触らない。
    pub fn prune_to_size_limit_locked(
        &self,
        max_bytes: u64,
        protected_src_path: &Path,
    ) -> rusqlite::Result<usize> {
        if max_bytes == 0 {
            return Ok(0);
        }

        let protected_key = crate::path_key::normalize(protected_src_path);
        let rows: Vec<(String, String, u64)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT src_path_key, cached_zip_path, cached_zip_size \
                 FROM converted_archives \
                 ORDER BY last_access_at ASC, converted_at ASC, src_path_key ASC",
            )?;
            stmt.query_map([], |r| {
                let size: i64 = r.get(2)?;
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    size.max(0) as u64,
                ))
            })?
            .flatten()
            .collect()
        };

        let mut total = rows
            .iter()
            .fold(0u64, |acc, (_, _, size)| acc.saturating_add(*size));
        if total <= max_bytes {
            return Ok(0);
        }

        let mut remove_keys = Vec::new();
        for (key, cached_path, size) in rows {
            if key == protected_key {
                continue;
            }
            remove_cache_file_and_dirs(Path::new(&cached_path));
            remove_keys.push(key);
            total = total.saturating_sub(size);
            if total <= max_bytes {
                break;
            }
        }

        if remove_keys.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare("DELETE FROM converted_archives WHERE src_path_key = ?1")?;
            for key in &remove_keys {
                stmt.execute(params![key])?;
            }
        }
        tx.commit()?;
        Ok(remove_keys.len())
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
            image_count      INTEGER NOT NULL, \
            password_required INTEGER NOT NULL DEFAULT 0\
         );",
    )?;
    let _ = conn.execute_batch(
        "ALTER TABLE converted_archives ADD COLUMN password_required INTEGER NOT NULL DEFAULT 0;",
    );
    Ok(())
}

fn format_to_db(f: ArchiveFormat) -> &'static str {
    match f {
        ArchiveFormat::Rar => "rar",
        ArchiveFormat::SevenZ => "7z",
        ArchiveFormat::Lzh => "lzh",
        // v1.3.0: 入れ子に非 ZIP アーカイブを含む ZIP の変換キャッシュ。
        // 旧バージョンの format_from_db は "zip" を None として無視するだけなので
        // ダウングレードしても DB 破壊にはならない (行は読み飛ばされる)。
        ArchiveFormat::Zip => "zip",
    }
}

fn format_from_db(s: &str) -> Option<ArchiveFormat> {
    match s {
        "rar" | "cbr" => Some(ArchiveFormat::Rar),
        "7z" | "cb7" => Some(ArchiveFormat::SevenZ),
        "lzh" | "lha" => Some(ArchiveFormat::Lzh),
        "zip" | "cbz" => Some(ArchiveFormat::Zip),
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

    #[test]
    fn schema_and_roundtrip() {
        // 2026-05-17: 旧版は `DATA_DIR.set(...).ok()` で OnceLock を直接叩いていたが、
        // (a) 2 回目以降の set は silently 失敗するので並列テストで古い (= 既に削除済み)
        //     temp dir を握ったまま動く、(b) TEST_OVERRIDE を使う他テストと並列実行すると
        //     `data_dir::get()` は TEST_OVERRIDE を優先するので DATA_DIR は無効化される、
        // という二重の問題があった (Codex P1 / 2026-05-17 指摘で発覚)。
        // `TestDataDirGuard` は `test_override_lock` で他の override 系テストと直列化し、
        // TEST_OVERRIDE 経由で temp dir を渡すので両方解決する。
        let _guard = crate::data_dir::TestDataDirGuard::new();
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

    #[test]
    fn password_required_cache_is_reusable_but_marked() {
        let g = crate::data_dir::TestDataDirGuard::new();
        let db = ArchiveCacheDb::open().unwrap();
        let src = g.path().join("secret.rar");
        let cached = g.path().join("secret.zip");
        std::fs::write(&src, b"rar").unwrap();
        std::fs::write(&cached, b"zip").unwrap();
        let meta = std::fs::metadata(&src).unwrap();
        let mtime = crate::ui_helpers::mtime_secs(&meta);
        let size = meta.len() as i64;

        db.record(&src, mtime, size, ArchiveFormat::Rar, &cached, 3, 1, true)
            .unwrap();

        assert_eq!(db.lookup(&src, mtime, size), Some(cached.clone()));
        let rows = db.list_all().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].format, Some(ArchiveFormat::Rar));
        assert_eq!(rows[0].format_raw, "rar");
        assert!(rows[0].password_required);
    }

    #[test]
    fn unknown_format_rows_are_listed_and_deletable() {
        let g = crate::data_dir::TestDataDirGuard::new();
        let db = ArchiveCacheDb::open().unwrap();
        let src = g.path().join("future.arc");
        std::fs::write(&src, b"archive").unwrap();
        let cached = db.reserve_cache_zip_path(&src).unwrap();
        std::fs::write(&cached, b"zip").unwrap();

        let meta = std::fs::metadata(&src).unwrap();
        let src_mtime = crate::ui_helpers::mtime_secs(&meta);
        let src_size = meta.len() as i64;
        let key = crate::path_key::normalize(&src);
        let now = now_secs();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO converted_archives \
                 (src_path_key, src_path, src_mtime, src_size, format, \
                  cached_zip_path, cached_zip_size, converted_at, last_access_at, image_count, password_required) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10)",
                params![
                    key,
                    src.to_string_lossy().to_string(),
                    src_mtime,
                    src_size,
                    "future-format",
                    cached.to_string_lossy().to_string(),
                    3i64,
                    now,
                    1i64,
                    0i64,
                ],
            )
            .unwrap();
        }

        let rows = db.list_all().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].format, None);
        assert_eq!(rows[0].format_raw, "future-format");
        assert!(rows[0].src_exists);

        db.delete_entry(&src).unwrap();
        assert!(!cached.exists());
        assert!(db.list_all().unwrap().is_empty());
    }

    #[test]
    fn prune_to_size_limit_removes_old_entries_but_keeps_protected() {
        let g = crate::data_dir::TestDataDirGuard::new();
        let db = ArchiveCacheDb::open().unwrap();

        let old_src = g.path().join("old.7z");
        let warm_src = g.path().join("warm.7z");
        let new_src = g.path().join("new.7z");
        std::fs::write(&old_src, b"old").unwrap();
        std::fs::write(&warm_src, b"warm").unwrap();
        std::fs::write(&new_src, b"new").unwrap();

        let old_cached = record_test_cache(&db, &old_src, 700);
        let warm_cached = record_test_cache(&db, &warm_src, 500);
        let new_cached = record_test_cache(&db, &new_src, 900);
        set_test_last_access(&db, &old_src, 10);
        set_test_last_access(&db, &warm_src, 20);
        set_test_last_access(&db, &new_src, 30);

        let _guard = db.begin_convert();
        let removed = db.prune_to_size_limit_locked(500, &new_src).expect("prune");
        drop(_guard);

        assert_eq!(removed, 2);
        assert!(!old_cached.exists());
        assert!(!warm_cached.exists());
        assert!(new_cached.exists(), "protected fresh cache must survive");
        assert_eq!(db.list_all().unwrap().len(), 1);
    }

    /// peek は lookup と同じ有効性判定 (mtime/size 一致 + キャッシュ実在) を行うが、
    /// `last_access_at` を更新しない (フォルダ一覧の一括参照で書き込みを発行しない)。
    #[test]
    fn peek_validates_without_touching_last_access() {
        let _guard = crate::data_dir::TestDataDirGuard::new();
        let db = ArchiveCacheDb::open().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("book.7z");
        std::fs::write(&src, b"archive").unwrap();
        let cached = record_test_cache(&db, &src, 100);
        set_test_last_access(&db, &src, 42);

        let meta = std::fs::metadata(&src).unwrap();
        let mtime = crate::ui_helpers::mtime_secs(&meta);
        let size = meta.len() as i64;

        // ヒット: cache ZIP パスが返る。
        assert_eq!(
            db.peek(&src, mtime, size).as_deref(),
            Some(cached.as_path())
        );
        // last_access_at が更新されていない (lookup と違い読み取り専用)。
        let entries = db.list_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].last_access_at, 42);

        // mtime 不一致 → None (掃除もしない: 行とファイルは残る)。
        assert!(db.peek(&src, mtime + 1, size).is_none());
        assert!(cached.exists());
        assert_eq!(db.list_all().unwrap().len(), 1);

        // キャッシュ ZIP が消えていたら None。
        std::fs::remove_file(&cached).unwrap();
        assert!(db.peek(&src, mtime, size).is_none());
    }

    fn record_test_cache(db: &ArchiveCacheDb, src: &Path, cached_size: usize) -> PathBuf {
        let cached = db.reserve_cache_zip_path(src).unwrap();
        std::fs::write(&cached, vec![b'x'; cached_size]).unwrap();
        let meta = std::fs::metadata(src).unwrap();
        db.record(
            src,
            crate::ui_helpers::mtime_secs(&meta),
            meta.len() as i64,
            ArchiveFormat::SevenZ,
            &cached,
            cached_size as i64,
            1,
            false,
        )
        .unwrap();
        cached
    }

    fn set_test_last_access(db: &ArchiveCacheDb, src: &Path, last_access_at: i64) {
        let key = crate::path_key::normalize(src);
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE converted_archives SET last_access_at = ?1, converted_at = ?1 \
             WHERE src_path_key = ?2",
            params![last_access_at, key],
        )
        .unwrap();
    }
}
