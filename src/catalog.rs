use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

const CATALOG_VERSION: &str = "2";
const PDF_LAYOUT_DIMS_META_KEY: &str = "pdf_layout_dims_version";
const PDF_LAYOUT_DIMS_VERSION: &str = "1";
pub const THUMB_LONG_SIDE: u32 = 512;

// -----------------------------------------------------------------------
// DB path helpers
// -----------------------------------------------------------------------

use crate::path_key;

/// `{cache_dir}/{xx}/{sha256}.db` の形式で DB ファイルパスを返す。
/// xx はハッシュ hex 先頭2文字（256サブフォルダに分散）。
pub fn db_path_for(cache_dir: &Path, folder_path: &Path) -> PathBuf {
    // 通常のサブフォルダは従来どおりドライブ文字を捨て、リムーバブルドライブの
    // レター変更でもキャッシュを引き継ぐ。一方でドライブルートだけは `C:\Photos`
    // と `D:\Photos` のような直下同名項目が同じ root catalog / 同じ basename key に
    // 衝突するため、ドライブ文字を保持して DB 自体を分離する。
    let normalized = if path_key::is_drive_or_share_root(folder_path) {
        path_key::normalize_keep_drive(folder_path)
    } else {
        path_key::normalize(folder_path)
    };
    let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    cache_dir.join(&hash[..2]).join(format!("{}.db", hash))
}

// -----------------------------------------------------------------------
// キャッシュエントリ
// -----------------------------------------------------------------------

#[derive(Clone)]
pub struct CacheEntry {
    pub mtime: i64,
    pub file_size: i64,
    pub jpeg_data: Vec<u8>,
    /// 元画像の寸法 (幅, 高さ)。通常画像 / archive entry はピクセル、PDF は page box を
    /// 1/1000 point で表すレイアウト寸法。いずれも thumbnail raster の寸法ではない。
    /// 旧バージョンで保存されたエントリには NULL が入るため Option で表現する。
    pub source_dims: Option<(u32, u32)>,
}

/// ZIP / 画像のみフォルダ / 変換対象アーカイブのページ数キャッシュ種別。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerPageKind {
    Folder = 1,
    Zip = 2,
    Archive = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerPageMeta {
    /// `None` は走査に成功したが、本として扱う対象ではなかったことを表す。
    pub page_count: Option<u32>,
}

/// 保存済みサムネのバイト列からヘッダのみで `(w, h)` を取り出す。
/// フォーマットは auto-detect (`with_guessed_format`)。これは旧バージョンが JPEG で
/// 保存していたエントリ ([`decode_thumb_to_color_image`] が "WebP or old JPEG" の
/// 両方を読んでいる) との互換性のため。フルデコードは走らない
/// (`ImageReader::into_dimensions` はチャンクヘッダだけを読む)。
pub fn decode_thumb_dims(data: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

// -----------------------------------------------------------------------
// CatalogDb
// -----------------------------------------------------------------------

pub struct CatalogDb {
    conn: Mutex<Connection>,
}

impl CatalogDb {
    /// cache_dir 配下の適切な場所に DB を開く（なければ作成）。
    /// サブディレクトリも自動作成する。
    pub fn open(cache_dir: &Path, folder_path: &Path) -> rusqlite::Result<Self> {
        let db_path = db_path_for(cache_dir, folder_path);
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut conn = Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        init_schema(&conn)?;
        migrate_pdf_layout_dims(&mut conn, folder_path)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 既存 catalog だけを読み取り専用で開く。
    ///
    /// 再帰フォルダ代表の cache-only 伝播では、キャッシュ削除後に空 DB を作り直すと
    /// 「キャッシュがある」ように見えてしまうため、ファイルが無い場合は `Ok(None)` を返す。
    /// 呼び出し元はサムネイル重 I/O worker に限定し、UI スレッドから cold open しないこと。
    pub fn open_existing_read_only(
        cache_dir: &Path,
        folder_path: &Path,
    ) -> rusqlite::Result<Option<Self>> {
        let db_path = db_path_for(cache_dir, folder_path);
        if !db_path.try_exists().unwrap_or(false) {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Some(Self {
            conn: Mutex::new(conn),
        }))
    }

    /// DB 内の全エントリを HashMap<filename, CacheEntry> として返す（一括 SELECT）。
    pub fn load_all(&self) -> rusqlite::Result<HashMap<String, CacheEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT filename, mtime, file_size, thumb_data, source_width, source_height \
             FROM thumbnails",
        )?;
        let mut map = HashMap::new();
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Option<u32>>(4)?,
                row.get::<_, Option<u32>>(5)?,
            ))
        })?;
        for item in iter.flatten() {
            let (filename, mtime, file_size, jpeg_data, src_w, src_h) = item;
            let source_dims = match (src_w, src_h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            };
            map.insert(
                filename,
                CacheEntry {
                    mtime,
                    file_size,
                    jpeg_data,
                    source_dims,
                },
            );
        }
        Ok(map)
    }

    /// 単一エントリのみ取り出す。`load_all` を呼ぶほどではないが特定 key だけ確認したい
    /// 場合用 (例: 仮想フォルダ進入時の親 catalog からの seed lookup)。
    pub fn load_one(&self, filename: &str) -> rusqlite::Result<Option<CacheEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT mtime, file_size, thumb_data, source_width, source_height \
             FROM thumbnails WHERE filename = ?1",
        )?;
        let mut iter = stmt.query_map(params![filename], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<u32>>(3)?,
                row.get::<_, Option<u32>>(4)?,
            ))
        })?;
        if let Some(item) = iter.next() {
            let (mtime, file_size, jpeg_data, src_w, src_h) = item?;
            let source_dims = match (src_w, src_h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            };
            return Ok(Some(CacheEntry {
                mtime,
                file_size,
                jpeg_data,
                source_dims,
            }));
        }
        Ok(None)
    }

    /// `filename` が `prefix` で始まるエントリのうち、mtime / size が最も新しいものを
    /// 1 件だけ返す。フォルダ代表サムネのように base key と `#pin:` 派生 key の
    /// どちらにも既存サムネが残り得る場合の cache-only 参照に使う。
    pub fn load_latest_with_prefix(
        &self,
        prefix: &str,
    ) -> rusqlite::Result<Option<(String, CacheEntry)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT filename, mtime, file_size, thumb_data, source_width, source_height \
             FROM thumbnails \
             WHERE substr(filename, 1, ?1) = ?2 \
             ORDER BY mtime DESC, file_size DESC, filename DESC \
             LIMIT 1",
        )?;
        let mut iter = stmt.query_map(params![prefix.chars().count() as i64, prefix], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Option<u32>>(4)?,
                row.get::<_, Option<u32>>(5)?,
            ))
        })?;
        if let Some(item) = iter.next() {
            let (filename, mtime, file_size, jpeg_data, src_w, src_h) = item?;
            let source_dims = match (src_w, src_h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            };
            return Ok(Some((
                filename,
                CacheEntry {
                    mtime,
                    file_size,
                    jpeg_data,
                    source_dims,
                },
            )));
        }
        Ok(None)
    }

    /// サムネイルを INSERT OR REPLACE で保存する。
    ///
    /// `width` / `height` はキャッシュされる WebP サムネイルの寸法、
    /// `source_dims` は元画像の寸法 (PDF は page box の 1/1000 point、未取得なら None)。
    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        width: u32,
        height: u32,
        source_dims: Option<(u32, u32)>,
        jpeg_data: &[u8],
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        let src_w: Option<u32> = source_dims.map(|(w, _)| w);
        let src_h: Option<u32> = source_dims.map(|(_, h)| h);
        conn.execute(
            "INSERT OR REPLACE INTO thumbnails \
             (filename, mtime, file_size, width, height, thumb_data, source_width, source_height) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                filename, mtime, file_size, width, height, jpeg_data, src_w, src_h
            ],
        )?;
        Ok(())
    }

    /// サムネバイト列のヘッダから `(w, h)` だけを取り出して `save` する薄いラッパ。
    /// `CacheEntry` には寸法フィールドが無いため、save 経由では `(w, h)` を呼び出し側で
    /// 用意する必要がある。仮想フォルダの seed / write-back のように「親 catalog から
    /// バイトをそのままミラーする」用途で繰り返し書きがちなので集約した。
    /// ヘッダのみ解析なのでフルデコードは走らない。
    ///
    /// 戻り値の `bool` は「実際に保存できたか」。`false` は「寸法を取り出せず保存を断念
    /// した」を意味する (= 壊れたバイト列)。呼び出し側はこれをもとに「cache_map にも
    /// 入れない」ことで、サムネ表示時に `Failed` 状態に陥るのを防げる。
    pub fn save_thumb_bytes(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        source_dims: Option<(u32, u32)>,
        jpeg_data: &[u8],
    ) -> rusqlite::Result<bool> {
        let Some((w, h)) = decode_thumb_dims(jpeg_data) else {
            // 寸法が取れない (= 壊れたバイト列) なら保存を断念。SQLite スキーマ上
            // width/height は NOT NULL なので 0 を入れると整合性が壊れる。
            return Ok(false);
        };
        self.save(filename, mtime, file_size, w, h, source_dims, jpeg_data)?;
        Ok(true)
    }

    /// 単一エントリを `filename` キーで削除する。該当行が無くてもエラーにしない。
    ///
    /// 用途: フォルダ代表ピンが Video を指していたが対応する `video_pins` の WebP が
    /// 消えた / 空になった場合、`folderthumb:{dir}#pin:...` のキャッシュ行を明示的に
    /// 削除して worker を auto-pick fallback に落とすため (Codex Phase C P2 指摘)。
    pub fn delete_one(&self, filename: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM thumbnails WHERE filename = ?1",
            params![filename],
        )?;
        Ok(())
    }

    /// `existing` に含まれないファイル名の行を削除する（削除済みファイルの掃除）。
    pub fn delete_missing(&self, existing: &HashSet<String>) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        let db_names: Vec<String> = {
            let mut stmt = conn.prepare("SELECT filename FROM thumbnails")?;
            stmt.query_map([], |r| r.get(0))?.flatten().collect()
        };
        for name in db_names {
            if !existing.contains(&name) {
                conn.execute("DELETE FROM thumbnails WHERE filename = ?1", params![name])?;
            }
        }
        Ok(())
    }

    // -------------------------------------------------------------------
    // PDF ページ数メタキャッシュ (v1.0.0)
    //
    // load_pdf_as_folder で Enter→ページ一覧の体感を瞬時にするため、PDFium による
    // PDF open + 構造解析 (warm 5-30ms / cold 100-1300ms) の結果をフォルダごとの
    // catalog DB に永続化する。lookup 時に mtime/file_size が一致すれば cache hit
    // とみなし、即座に N セルの placeholder grid を立てる (= 824ms 待ちを回避)。
    //
    // `password_required` は「最後に成功した enumerate がパスワード保護下だったか」
    // を記録する。パスワード保存なしで cache hit のグリッドを見せると、後で保存
    // パスワードが削除された場合に保護を bypass してしまうため、cache 利用前に
    // `password_required==1 && pdf_passwords にエントリ無し` の組み合わせを
    // 明示的に弾く (Codex P1 対応)。
    // -------------------------------------------------------------------

    /// PDF メタキャッシュをルックアップする。
    ///
    /// `(filename, mtime, file_size)` が完全に一致した場合のみ `Some((page_count,
    /// password_required))` を返す。mtime/file_size 不一致は cache miss (None)。
    /// `password_required == true` の場合、呼び出し側は更に「保存パスワードがある」
    /// ことを確認してから cache を利用すること。
    pub fn get_pdf_meta(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
    ) -> rusqlite::Result<Option<(u32, bool)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT page_count, password_required FROM pdf_meta \
             WHERE filename = ?1 AND mtime = ?2 AND file_size = ?3",
        )?;
        let result = stmt
            .query_row(params![filename, mtime, file_size], |r| {
                let page_count: i64 = r.get(0)?;
                let pw_req: i64 = r.get(1)?;
                Ok((page_count.max(0) as u32, pw_req != 0))
            })
            .ok();
        Ok(result)
    }

    /// PDF メタキャッシュを INSERT OR REPLACE する。
    /// `page_count == 0` のような無効値もそのまま記録 (= 後で stale 検出に使える)。
    /// `password_required` は呼び出し側が「この PDF 固有の保存パスワードが必要」と
    /// 確信している場合だけ true を渡すこと。session 経由の暫定パスワードでは
    /// `set_pdf_meta_thumb` 側を使う (既存値を保持する)。
    pub fn set_pdf_meta(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
        password_required: bool,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO pdf_meta \
             (filename, mtime, file_size, page_count, password_required) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                filename,
                mtime,
                file_size,
                page_count as i64,
                if password_required { 1i64 } else { 0i64 },
            ],
        )?;
        Ok(())
    }

    /// 既存 `pdf_meta` 行が **同じ mtime/file_size のとき** のみ `page_count` を更新する。
    /// それ以外 (新規行 / mtime or file_size 変化) は no-op。
    ///
    /// 用途: password=Some だが「この PDF の保存パスワードがある」とは確信できない
    /// 経路 (= session-level の `pdf_current_password` が居座っているだけかもしれない、
    /// or ユーザーがダイアログで入力したが「保存しない」を選んだ)。
    ///
    /// **mtime/file_size 一致条件の理由 (Codex P1 round 3 対応)**:
    /// 単純な `UPDATE WHERE filename=?` だと、stale な「非暗号化版」の行が、暗号化版に
    /// ファイル置換された後の UPDATE で新 mtime/size を被って lookup hit するようになる
    /// → password_required=0 が保持されたまま placeholder で bypass される。
    /// mtime/file_size が既存行と一致するときだけ更新することで、
    ///   - ファイル不変 (= mtime/size 同じ) → 既存 password_required の確信を保ったまま
    ///     page_count を verify update (実質 no-op になることが多い)
    ///   - ファイル変化 (= mtime/size 違う) → no-op、stale 行はそのまま放置。次回 lookup
    ///     で mtime mismatch して miss するので、確信あり経路で改めて書き直される
    /// が成立する。
    pub fn set_pdf_meta_thumb(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE pdf_meta \
             SET page_count = ?4 \
             WHERE filename = ?1 AND mtime = ?2 AND file_size = ?3",
            params![filename, mtime, file_size, page_count as i64],
        )?;
        Ok(())
    }

    /// `password 不要が確信できる場合**用の UPSERT。
    /// 新規行・既存行とも `password_required=0` で書き込み、
    /// `page_count`/`mtime`/`file_size` を更新する。
    ///
    /// 用途: サムネワーカーが `pdf_password=None` で render に成功した場合 (=
    /// PDFium 側で「パスワード不要」と判明した = 確信あり)。
    ///
    /// **既存行の `password_required` も上書きする理由 (review #1 対応)**:
    /// 呼び出し側の不変条件「password=None で render 成功」が成立しているので、
    /// (filename, mtime, file_size) の組合せで指している今のファイルは確実に
    /// 非保護。既存行 `password_required=1` を保持してしまうと、保護版を
    /// 非保護版に差し替えた場合に永続的に「保護扱い」が残り、placeholder grid が
    /// 表示できず無意味なパスワード入力ダイアログを毎回開く羽目になる。
    /// 「render が None で通った時点で password_required は 0 と判明した」事実を
    /// そのまま反映する。
    pub fn set_pdf_meta_safe(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO pdf_meta \
             (filename, mtime, file_size, page_count, password_required) \
             VALUES (?1, ?2, ?3, ?4, 0) \
             ON CONFLICT(filename) DO UPDATE SET \
               mtime = excluded.mtime, \
               file_size = excluded.file_size, \
               page_count = excluded.page_count, \
               password_required = 0",
            params![filename, mtime, file_size, page_count as i64],
        )?;
        Ok(())
    }

    /// ZIP / 画像のみフォルダ / 変換対象アーカイブのページ数を、内容 identity と判定設定
    /// fingerprint が完全一致するときだけ返す。失敗結果は保存せず、`page_count=NULL` はフォルダを
    /// 正常に走査した結果「本として扱う対象外」だったことを表す。
    pub fn get_container_page_meta(
        &self,
        filename: &str,
        kind: ContainerPageKind,
        mtime: i64,
        file_size: i64,
        fingerprint: i64,
    ) -> rusqlite::Result<Option<ContainerPageMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT page_count FROM container_page_meta \
             WHERE filename = ?1 AND kind = ?2 AND mtime = ?3 \
               AND file_size = ?4 AND fingerprint = ?5",
        )?;
        stmt.query_row(
            params![filename, kind as i64, mtime, file_size, fingerprint],
            |row| {
                let count: Option<i64> = row.get(0)?;
                Ok(ContainerPageMeta {
                    page_count: count.map(|value| value.max(0) as u32),
                })
            },
        )
        .optional()
    }

    pub fn set_container_page_meta(
        &self,
        filename: &str,
        kind: ContainerPageKind,
        mtime: i64,
        file_size: i64,
        fingerprint: i64,
        page_count: Option<u32>,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO container_page_meta \
             (filename, kind, mtime, file_size, fingerprint, page_count) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(filename, kind) DO UPDATE SET \
               mtime = excluded.mtime, file_size = excluded.file_size, \
               fingerprint = excluded.fingerprint, page_count = excluded.page_count",
            params![
                filename,
                kind as i64,
                mtime,
                file_size,
                fingerprint,
                page_count.map(i64::from),
            ],
        )?;
        Ok(())
    }
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS thumbnails (
             filename       TEXT    NOT NULL PRIMARY KEY,
             mtime          INTEGER NOT NULL,
             file_size      INTEGER NOT NULL,
             width          INTEGER NOT NULL,
             height         INTEGER NOT NULL,
             thumb_data     BLOB    NOT NULL,
             source_width   INTEGER,
             source_height  INTEGER
         );
         CREATE TABLE IF NOT EXISTS pdf_meta (
             filename          TEXT    NOT NULL PRIMARY KEY,
             mtime             INTEGER NOT NULL,
             file_size         INTEGER NOT NULL,
             page_count        INTEGER NOT NULL,
             password_required INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS container_page_meta (
             filename       TEXT    NOT NULL,
             kind           INTEGER NOT NULL,
             mtime          INTEGER NOT NULL,
             file_size      INTEGER NOT NULL,
             fingerprint    INTEGER NOT NULL,
             page_count     INTEGER,
             PRIMARY KEY(filename, kind)
         );",
    )?;
    // 非破壊マイグレーション。open ごとの ALTER 失敗ログを避け、並行 open が同時に
    // missing を観測した場合だけ duplicate column を idempotent success として扱う。
    add_thumbnail_column_if_missing(
        conn,
        "source_width",
        "ALTER TABLE thumbnails ADD COLUMN source_width INTEGER",
    )?;
    add_thumbnail_column_if_missing(
        conn,
        "source_height",
        "ALTER TABLE thumbnails ADD COLUMN source_height INTEGER",
    )?;

    // バージョン不一致（スキーマ変更）の場合は全削除して再生成
    let version: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
            r.get(0)
        })
        .ok();
    if version.as_deref() != Some(CATALOG_VERSION) {
        conn.execute_batch("DELETE FROM thumbnails;")?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('version', ?1)",
            params![CATALOG_VERSION],
        )?;
    }
    Ok(())
}

/// Released catalogs stored PDF thumbnail raster dimensions in `source_*`.
/// They cannot be repaired from the WebP because the page-box precision has
/// already been discarded, so invalidate only PDF-derived thumbnail rows once.
///
/// A catalog whose owner is a PDF contains its virtual `page_NNNN` rows only;
/// ordinary folder catalogs may contain `pdfthumb:` representative rows beside
/// unrelated image/ZIP entries, which must remain intact.
fn migrate_pdf_layout_dims(conn: &mut Connection, folder_path: &Path) -> rusqlite::Result<()> {
    let current: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [PDF_LAYOUT_DIMS_META_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if current.as_deref() == Some(PDF_LAYOUT_DIMS_VERSION) {
        return Ok(());
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Another opener may have completed the migration while this connection
    // waited for the write lock. Recheck under the transaction before deleting.
    let version: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [PDF_LAYOUT_DIMS_META_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if version.as_deref() == Some(PDF_LAYOUT_DIMS_VERSION) {
        return tx.commit();
    }
    let is_pdf_catalog = folder_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    if is_pdf_catalog {
        tx.execute("DELETE FROM thumbnails", [])?;
    } else {
        tx.execute(
            "DELETE FROM thumbnails WHERE filename LIKE 'pdfthumb:%'",
            [],
        )?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![PDF_LAYOUT_DIMS_META_KEY, PDF_LAYOUT_DIMS_VERSION],
    )?;
    tx.commit()
}

fn add_thumbnail_column_if_missing(
    conn: &Connection,
    column: &str,
    alter_sql: &str,
) -> rusqlite::Result<()> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('thumbnails') WHERE name = ?1 LIMIT 1",
            [column],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        return Ok(());
    }
    match conn.execute(alter_sql, []) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("duplicate column name") =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

// -----------------------------------------------------------------------
// WebP エンコード・デコードヘルパー
// -----------------------------------------------------------------------

/// 画像を `long_side` px にリサイズし、ロッシー WebP でエンコードする。
/// `quality` は 0.0–100.0 (JPEG の quality と同等の意味)。
/// 戻り値: (webp_bytes, width, height)
///
/// リサイズは SIMD 実装の `fast_image_resize` を Lanczos3 で使用する
/// (image crate のスカラー Lanczos3 より 3-5 倍速い)。
pub fn encode_thumb_webp(
    img: &image::DynamicImage,
    long_side: u32,
    quality: f32,
) -> Option<(Vec<u8>, u32, u32)> {
    let thumb = crate::fast_resize::resize_dynamic_fit(
        img,
        long_side,
        long_side,
        crate::fast_resize::Quality::Lanczos3,
    );
    let rgb = thumb.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let encoder = webp::Encoder::from_rgb(rgb.as_raw(), w, h);
    let webp_data = encoder.encode(quality.clamp(1.0, 100.0));
    Some((webp_data.to_vec(), w, h))
}

/// キャッシュされたサムネイル (WebP あるいは旧 JPEG) を egui::ColorImage にデコードする。
/// `image::load_from_memory` が自動でフォーマット判定するため両対応。
pub fn decode_thumb_to_color_image(data: &[u8]) -> Option<egui::ColorImage> {
    let (w, h, rgba) = decode_thumb_to_rgba(data)?;
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        &rgba,
    ))
}

/// `image::load_from_memory` でデコードして RGBA8 + (w, h) を返す。
/// `decode_thumb_to_color_image` と動画タイル サムネ cache の WebP 復元で共用。
pub fn decode_thumb_to_rgba(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(data).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Some((w, h, rgba.into_raw()))
}

/// キャッシュディレクトリのデフォルト位置（DATA_DIR\cache）
pub fn default_cache_dir() -> PathBuf {
    crate::data_dir::get().join("cache")
}

// -----------------------------------------------------------------------
// キャッシュ管理ユーティリティ
// -----------------------------------------------------------------------

/// cache_dir 配下の .db ファイル数と合計バイト数を返す。
pub fn cache_stats(cache_dir: &Path) -> (usize, u64) {
    let mut count = 0usize;
    let mut total_bytes = 0u64;
    collect_db_files(cache_dir, &mut |meta| {
        count += 1;
        total_bytes += meta.len();
    });
    (count, total_bytes)
}

/// cache_dir 配下で最終更新時刻が `days` 日以上前の .db ファイルを削除する。
/// 削除したファイル数を返す。
pub fn delete_old_cache(cache_dir: &Path, days: u64) -> usize {
    let now = std::time::SystemTime::now();
    let threshold = std::time::Duration::from_secs(days * 24 * 3600);
    let mut deleted = 0usize;
    collect_db_paths(cache_dir, &mut |path, meta| {
        let age = meta
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .unwrap_or(std::time::Duration::ZERO);
        if age >= threshold {
            if std::fs::remove_file(path).is_ok() {
                deleted += 1;
            }
        }
    });
    deleted
}

/// cache_dir 配下の .db ファイルをすべて削除する。
/// 削除したファイル数を返す。
pub fn delete_all_cache(cache_dir: &Path) -> usize {
    let mut deleted = 0usize;
    collect_db_paths(cache_dir, &mut |path, _| {
        if std::fs::remove_file(path).is_ok() {
            deleted += 1;
        }
    });
    deleted
}

/// cache_dir 配下の .db ファイルのパスとメタデータを列挙してコールバックを呼ぶ。
fn collect_db_paths(cache_dir: &Path, cb: &mut impl FnMut(&Path, std::fs::Metadata)) {
    let Ok(top) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for entry in top.flatten() {
        // per-entry GetFileAttributes syscall を避けるため file_type を 1 回取る
        // (docs/ui-responsiveness.md §4)。キャッシュ全走査は数千フォルダ規模になるので効く。
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let sub = entry.path();
        let Ok(sub_entries) = std::fs::read_dir(&sub) else {
            continue;
        };
        for file in sub_entries.flatten() {
            let p = file.path();
            if p.extension().and_then(|e| e.to_str()) == Some("db") {
                if let Ok(meta) = file.metadata() {
                    cb(&p, meta);
                }
            }
        }
    }
}

/// collect_db_paths の統計専用バリアント（パス不要）。
fn collect_db_files(cache_dir: &Path, cb: &mut impl FnMut(std::fs::Metadata)) {
    collect_db_paths(cache_dir, &mut |_, meta| cb(meta));
}

// -----------------------------------------------------------------------
// テスト
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex;

    /// テスト用: in-memory SQLite で CatalogDb を作成する。
    fn open_in_memory() -> CatalogDb {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .unwrap();
        init_schema(&conn).unwrap();
        CatalogDb {
            conn: Mutex::new(conn),
        }
    }

    // -- db_path_for --

    #[test]
    fn db_path_for_deterministic() {
        let cache = Path::new(r"C:\cache");
        let folder = Path::new(r"D:\photos\2024");
        let a = db_path_for(cache, folder);
        let b = db_path_for(cache, folder);
        assert_eq!(a, b);
    }

    #[test]
    fn db_path_for_different_paths() {
        let cache = Path::new(r"C:\cache");
        let a = db_path_for(cache, Path::new(r"D:\photos\2024"));
        let b = db_path_for(cache, Path::new(r"D:\photos\2025"));
        assert_ne!(a, b);
    }

    #[test]
    fn db_path_for_case_insensitive() {
        let cache = Path::new(r"C:\cache");
        let a = db_path_for(cache, Path::new(r"C:\Photos\Vacation"));
        let b = db_path_for(cache, Path::new(r"D:\photos\vacation"));
        // ドライブ文字は除去され、小文字化されるので同じパスになるはず
        assert_eq!(a, b);
    }

    #[test]
    fn db_path_for_drive_roots_keeps_drive_letter() {
        let cache = Path::new(r"C:\cache");
        let c = db_path_for(cache, Path::new(r"C:\"));
        let d = db_path_for(cache, Path::new(r"D:\"));
        assert_ne!(c, d, "ドライブルート catalog は直下同名項目の衝突を避ける");
    }

    #[test]
    fn db_path_for_non_root_still_ignores_drive_letter() {
        let cache = Path::new(r"C:\cache");
        let c = db_path_for(cache, Path::new(r"C:\Photos"));
        let d = db_path_for(cache, Path::new(r"D:\photos"));
        assert_eq!(c, d, "非 root は従来どおりドライブレター変更に追従する");
    }

    #[test]
    fn db_path_for_structure() {
        let cache = Path::new(r"C:\cache");
        let result = db_path_for(cache, Path::new(r"D:\test"));
        let result_str = result.to_string_lossy();
        // {cache_dir}/{xx}/{hash}.db の形式
        assert!(result_str.starts_with(r"C:\cache\"));
        assert!(result_str.ends_with(".db"));
        // xx サブディレクトリが2文字の hex
        let relative = result.strip_prefix(cache).unwrap();
        let components: Vec<_> = relative.components().collect();
        assert_eq!(components.len(), 2); // xx/ と hash.db
    }

    // -- CatalogDb schema --

    #[test]
    fn catalog_open_and_schema() {
        let db = open_in_memory();
        let conn = db.conn.lock().unwrap();
        // meta テーブルにバージョンが記録されているか
        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, CATALOG_VERSION);
    }

    #[test]
    fn pdf_layout_migration_rebuilds_legacy_page_rows_once() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let pdf_path = tmp.path().join("book.pdf");
        let db = CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        db.save("page_0000", 1, 10, 327, 473, Some((327, 473)), b"legacy")
            .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM meta WHERE key = ?1",
                [PDF_LAYOUT_DIMS_META_KEY],
            )
            .unwrap();
        drop(db);

        let migrated = CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        assert!(migrated.load_all().unwrap().is_empty());
        migrated
            .save(
                "page_0000",
                1,
                10,
                327,
                473,
                Some((595_276, 841_890)),
                b"fixed",
            )
            .unwrap();
        drop(migrated);

        let reopened = CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        assert_eq!(
            reopened.load_all().unwrap()["page_0000"].source_dims,
            Some((595_276, 841_890)),
            "the migration marker must preserve regenerated rows on later opens"
        );
    }

    #[test]
    fn pdf_layout_migration_keeps_non_pdf_rows_in_folder_catalogs() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let folder = tmp.path().join("photos");
        let db = CatalogDb::open(&cache_dir, &folder).unwrap();
        db.save("image.jpg", 1, 10, 8, 8, Some((4000, 3000)), b"image")
            .unwrap();
        db.save(
            "pdfthumb:book.pdf",
            1,
            10,
            8,
            8,
            Some((327, 473)),
            b"legacy-pdf",
        )
        .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM meta WHERE key = ?1",
                [PDF_LAYOUT_DIMS_META_KEY],
            )
            .unwrap();
        drop(db);

        let migrated = CatalogDb::open(&cache_dir, &folder).unwrap();
        let rows = migrated.load_all().unwrap();
        assert!(rows.contains_key("image.jpg"));
        assert!(!rows.contains_key("pdfthumb:book.pdf"));
    }

    // -- CatalogDb CRUD --

    #[test]
    fn catalog_save_and_load_all() {
        let db = open_in_memory();
        db.save(
            "test.jpg",
            1000,
            2048,
            256,
            192,
            Some((4000, 3000)),
            b"fake_webp",
        )
        .unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        let entry = &map["test.jpg"];
        assert_eq!(entry.mtime, 1000);
        assert_eq!(entry.file_size, 2048);
        assert_eq!(entry.jpeg_data, b"fake_webp");
        assert_eq!(entry.source_dims, Some((4000, 3000)));
    }

    #[test]
    fn catalog_save_overwrites() {
        let db = open_in_memory();
        db.save("img.jpg", 100, 500, 128, 96, None, b"data1")
            .unwrap();
        db.save("img.jpg", 200, 600, 128, 96, None, b"data2")
            .unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map["img.jpg"].mtime, 200);
        assert_eq!(map["img.jpg"].jpeg_data, b"data2");
    }

    #[test]
    fn catalog_source_dims_none() {
        let db = open_in_memory();
        db.save("no_dims.jpg", 100, 500, 128, 96, None, b"data")
            .unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map["no_dims.jpg"].source_dims, None);
    }

    #[test]
    fn catalog_delete_missing() {
        let db = open_in_memory();
        db.save("keep.jpg", 100, 500, 128, 96, None, b"a").unwrap();
        db.save("remove.jpg", 200, 600, 128, 96, None, b"b")
            .unwrap();
        db.save("also_remove.jpg", 300, 700, 128, 96, None, b"c")
            .unwrap();

        let existing: HashSet<String> = ["keep.jpg".to_string()].into_iter().collect();
        db.delete_missing(&existing).unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("keep.jpg"));
    }

    #[test]
    fn catalog_delete_one_removes_only_target() {
        let db = open_in_memory();
        db.save("a.jpg", 1, 10, 8, 8, None, b"a").unwrap();
        db.save("b.jpg", 1, 10, 8, 8, None, b"b").unwrap();
        db.delete_one("a.jpg").unwrap();
        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("b.jpg"));
        assert!(!map.contains_key("a.jpg"));
        // 二度目の delete (存在しないキー) もエラーにしない
        db.delete_one("a.jpg").unwrap();
        db.delete_one("never_existed.jpg").unwrap();
    }

    #[test]
    fn catalog_load_latest_with_prefix_picks_newest_matching_pin_entry() {
        let db = open_in_memory();
        db.save("folderthumb:child", 10, 1, 8, 8, None, b"base")
            .unwrap();
        db.save(
            "folderthumb:child#pin:image|cover|-|-|20|2",
            20,
            2,
            8,
            8,
            None,
            b"pin",
        )
        .unwrap();
        db.save("folderthumb:child-other", 99, 9, 8, 8, None, b"other")
            .unwrap();

        let (filename, entry) = db
            .load_latest_with_prefix("folderthumb:child#pin:")
            .unwrap()
            .expect("narrow prefix hit");
        assert_eq!(filename, "folderthumb:child#pin:image|cover|-|-|20|2");
        assert_eq!(entry.jpeg_data, b"pin");
        assert!(db.load_latest_with_prefix("missing:").unwrap().is_none());
    }

    #[test]
    fn catalog_column_migration_runs_once_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE thumbnails (
                filename TEXT NOT NULL PRIMARY KEY,
                mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                width INTEGER NOT NULL,
                height INTEGER NOT NULL,
                thumb_data BLOB NOT NULL
            );",
        )
        .unwrap();

        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();
        let migrated: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('thumbnails')
                 WHERE name IN ('source_width', 'source_height')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migrated, 2);
    }

    #[test]
    fn catalog_version_mismatch_clears() {
        // 1) DB を作成してデータを保存
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO thumbnails (filename, mtime, file_size, width, height, thumb_data) \
             VALUES ('old.jpg', 1, 1, 1, 1, X'00')",
            [],
        )
        .unwrap();
        // データが存在することを確認
        let count: i64 = conn
            .query_row("SELECT count(*) FROM thumbnails", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // 2) バージョンを不正な値に書き換え
        conn.execute(
            "UPDATE meta SET value = 'old_version' WHERE key = 'version'",
            [],
        )
        .unwrap();

        // 3) init_schema を再度呼ぶとバージョン不一致で全削除されるはず
        init_schema(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM thumbnails", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn container_page_meta_roundtrip_and_identity_invalidation() {
        let db = open_in_memory();
        db.set_container_page_meta("book.zip", ContainerPageKind::Zip, 100, 2_048, 0, Some(123))
            .unwrap();
        db.set_container_page_meta(
            "book.rar",
            ContainerPageKind::Archive,
            300,
            4_096,
            77,
            Some(45),
        )
        .unwrap();

        assert_eq!(
            db.get_container_page_meta("book.zip", ContainerPageKind::Zip, 100, 2_048, 0)
                .unwrap(),
            Some(ContainerPageMeta {
                page_count: Some(123)
            })
        );
        assert_eq!(
            db.get_container_page_meta("book.zip", ContainerPageKind::Zip, 101, 2_048, 0)
                .unwrap(),
            None,
            "mtime が変われば再走査する"
        );
        assert_eq!(
            db.get_container_page_meta("book.zip", ContainerPageKind::Zip, 100, 4_096, 0)
                .unwrap(),
            None,
            "サイズが変われば再走査する"
        );
        assert_eq!(
            db.get_container_page_meta("book.zip", ContainerPageKind::Folder, 100, 2_048, 0)
                .unwrap(),
            None,
            "同名でもコンテナ種別を混同しない"
        );
        assert_eq!(
            db.get_container_page_meta("book.rar", ContainerPageKind::Archive, 300, 4_096, 77,)
                .unwrap(),
            Some(ContainerPageMeta {
                page_count: Some(45)
            }),
            "変換対象アーカイブのページ数を独立して保存する"
        );
    }

    #[test]
    fn container_page_meta_preserves_non_book_and_fingerprint() {
        let db = open_in_memory();
        db.set_container_page_meta("pictures", ContainerPageKind::Folder, 200, 0, 77, None)
            .unwrap();

        assert_eq!(
            db.get_container_page_meta("pictures", ContainerPageKind::Folder, 200, 0, 77)
                .unwrap(),
            Some(ContainerPageMeta { page_count: None }),
            "走査済みの対象外フォルダは NULL として区別する"
        );
        assert_eq!(
            db.get_container_page_meta("pictures", ContainerPageKind::Folder, 200, 0, 78)
                .unwrap(),
            None,
            "判定設定が変われば再走査する"
        );
    }

    // -- WebP encode/decode --

    #[test]
    fn encode_thumb_webp_basic() {
        // 小さな 4x4 テスト画像を生成
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(4, 4, |x, y| {
            image::Rgb([(x * 60) as u8, (y * 60) as u8, 128])
        }));
        let result = encode_thumb_webp(&img, 4, 75.0);
        assert!(result.is_some());
        let (data, w, h) = result.unwrap();
        assert!(!data.is_empty());
        assert!(w <= 4 && h <= 4);
    }

    /// `collect_db_paths` が `cache_dir/<sub>/*.db` を網羅すること。
    /// `cache_dir/file.db` (top-level) は subdir でないので **無視**、
    /// 非 .db ファイル / 余計なフォルダの中の非 .db も無視。
    /// docs/ui-responsiveness.md §4 (file_type 経由) との整合を機能面から保証する。
    #[test]
    fn collect_db_paths_enumerates_only_subdir_db_files() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        // sub1: foo.db + readme.txt
        let sub1 = cache_dir.join("sub1");
        std::fs::create_dir_all(&sub1).unwrap();
        std::fs::write(sub1.join("foo.db"), b"x").unwrap();
        std::fs::write(sub1.join("readme.txt"), b"x").unwrap();
        // sub2: bar.db
        let sub2 = cache_dir.join("sub2");
        std::fs::create_dir_all(&sub2).unwrap();
        std::fs::write(sub2.join("bar.db"), b"x").unwrap();
        // top-level の loose db (subdir に居ない) は拾わない
        std::fs::write(cache_dir.join("loose.db"), b"x").unwrap();
        // 空サブフォルダは無害
        std::fs::create_dir_all(cache_dir.join("empty_sub")).unwrap();

        let mut found: Vec<String> = Vec::new();
        super::collect_db_paths(&cache_dir, &mut |p, _meta| {
            found.push(
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string(),
            );
        });
        found.sort();
        assert_eq!(
            found,
            vec!["bar.db".to_string(), "foo.db".to_string()],
            "subdir 配下の .db のみ列挙、top-level loose.db は無視"
        );
    }

    /// `collect_db_paths` は cache_dir 自体が存在しない場合に panic せず、
    /// 単にコールバックを呼ばずに return する (`std::fs::read_dir` Err 時の規約)。
    #[test]
    fn collect_db_paths_handles_missing_cache_dir() {
        let temp = tempfile::TempDir::new().unwrap();
        let nonexistent = temp.path().join("does_not_exist");
        let mut count = 0usize;
        super::collect_db_paths(&nonexistent, &mut |_, _| count += 1);
        assert_eq!(count, 0, "missing cache_dir なら空列挙");
    }

    /// 大量サブフォルダ (200 件) でも全 .db ファイルを取りこぼさず列挙する。
    /// 実時間 assert は flaky になるので、件数だけ厳密に確認 (file_type 経路で
    /// per-entry syscall が発生していないことの間接担保)。
    #[test]
    fn collect_db_paths_handles_many_subfolders() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        for i in 0..200 {
            let sub = cache_dir.join(format!("s{i:03}"));
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("a.db"), b"x").unwrap();
        }
        let mut count = 0usize;
        super::collect_db_paths(&cache_dir, &mut |_, _| count += 1);
        assert_eq!(count, 200, "200 件全部列挙");
    }

    /// 0.8.2 で `decode_thumb_dims` を WebP 固定から `with_guessed_format()` auto-detect
    /// に変更した回帰ガード。ここで JPEG が読めなくなると、旧バージョンが JPEG で書いた
    /// 親 catalog エントリから seed/writeback できなくなる (= 仮想フォルダの初回 thumb
    /// が永続的に失われる)。
    #[test]
    fn decode_thumb_dims_reads_webp_jpeg_and_rejects_garbage() {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(8, 6, |x, y| {
            image::Rgb([(x * 30) as u8, (y * 40) as u8, 200])
        }));

        // WebP (現行フォーマット): 寸法を返す
        let (webp_bytes, _, _) = encode_thumb_webp(&img, 8, 75.0).expect("webp encode ok");
        assert_eq!(decode_thumb_dims(&webp_bytes), Some((8, 6)));

        // JPEG (旧バージョンが書いていた形式): 寸法を返す
        let mut jpeg_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut jpeg_bytes),
            image::ImageFormat::Jpeg,
        )
        .expect("jpeg encode");
        assert_eq!(decode_thumb_dims(&jpeg_bytes), Some((8, 6)));

        // 破損データ: None。空バイト列・テキスト・WebP magic だけ・短縮 JPEG いずれも reject。
        assert_eq!(decode_thumb_dims(&[]), None);
        assert_eq!(decode_thumb_dims(b"NOT-AN-IMAGE-AT-ALL"), None);
        // RIFF/WEBP magic の手前 12 バイトだけ (本体なし)
        assert_eq!(
            decode_thumb_dims(b"RIFF\x00\x00\x00\x00WEBP"),
            None,
            "header だけで本体なし → None"
        );
        // JPEG SOI のみ (SOF0 まで届かない)
        assert_eq!(decode_thumb_dims(b"\xFF\xD8\xFF\xE0"), None);
    }

    // -- pdf_meta --

    #[test]
    fn pdf_meta_set_and_get_roundtrip() {
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();

        let result = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(result, Some((32, false)));
    }

    #[test]
    fn pdf_meta_mtime_mismatch_returns_none() {
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();

        // mtime が変わったら cache miss
        let result = db.get_pdf_meta("foo.pdf", 1001, 2048).unwrap();
        assert_eq!(result, None, "mtime 変化で cache miss");
    }

    #[test]
    fn pdf_meta_file_size_mismatch_returns_none() {
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();

        // file_size が変わったら cache miss
        let result = db.get_pdf_meta("foo.pdf", 1000, 4096).unwrap();
        assert_eq!(result, None, "file_size 変化で cache miss");
    }

    #[test]
    fn pdf_meta_password_required_flag_preserved() {
        let db = open_in_memory();
        db.set_pdf_meta("locked.pdf", 100, 500, 8, true).unwrap();

        let result = db.get_pdf_meta("locked.pdf", 100, 500).unwrap();
        assert_eq!(result, Some((8, true)));
    }

    #[test]
    fn pdf_meta_insert_or_replace() {
        let db = open_in_memory();
        // 同じ filename で 2 回 set → 2 回目で上書き
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 100, true).unwrap();

        let result = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(result, Some((100, true)), "2 回目の値が残る");
    }

    #[test]
    fn pdf_meta_get_missing_returns_none() {
        let db = open_in_memory();
        let result = db.get_pdf_meta("nonexistent.pdf", 1000, 2048).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn pdf_meta_does_not_affect_thumbnails_table() {
        // pdf_meta テーブルと thumbnails テーブルが独立していることを確認
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();

        let all = db.load_all().unwrap();
        assert!(all.is_empty(), "thumbnails は空のまま");
    }

    #[test]
    fn pdf_meta_thumb_preserves_password_required_flag() {
        // unknown 経路 (session-only pw など) の verify update が password_required を
        // 消さないこと (mtime/size 一致の場合のみ page_count update が走る)
        let db = open_in_memory();
        // 既存: パスワード必須として記録済み
        db.set_pdf_meta("locked.pdf", 1000, 2048, 32, true).unwrap();
        // 同じ mtime/size での unknown 経路 update (page_count もたまたま同じ)
        db.set_pdf_meta_thumb("locked.pdf", 1000, 2048, 32).unwrap();

        let result = db.get_pdf_meta("locked.pdf", 1000, 2048).unwrap();
        assert_eq!(
            result,
            Some((32, true)),
            "password_required=true が保持される"
        );
    }

    #[test]
    fn pdf_meta_thumb_does_not_insert_new_row() {
        // **Codex P1 round 2 対応**: 新規 PDF (= まだ pdf_meta 行が無い) で
        // unknown 経路 (set_pdf_meta_thumb) を呼んでも、false-default 行を作らない。
        // 保護 PDF を「パスワード入力したが保存しない」で開いたケースで、永続的に
        // 「非保護」と記録されてしまう bypass を防ぐ。
        let db = open_in_memory();
        db.set_pdf_meta_thumb("new.pdf", 500, 1024, 16).unwrap();

        let result = db.get_pdf_meta("new.pdf", 500, 1024).unwrap();
        assert_eq!(result, None, "新規行は作られない (UPDATE only)");
    }

    #[test]
    fn pdf_meta_thumb_does_not_promote_stale_row() {
        // **Codex P1 round 3 対応**: 旧 stale 行 (例: 非暗号化として cache 済み) を、
        // ファイル置換後 (暗号化版、新 mtime/size) の unknown 経路 update で
        // 新 mtime/size に上書きすると password_required=0 のまま昇格してしまう。
        // mtime/file_size 一致条件で no-op にする実装で防止する。
        let db = open_in_memory();
        // 旧: 非暗号化として記録 (mtime=1000, size=2000)
        db.set_pdf_meta("foo.pdf", 1000, 2000, 10, false).unwrap();
        // ファイル更新後、ユーザーが新版を session pw で開いて unknown 経路 update が来た
        // (新 mtime=2000, size=3000) — 旧 stale 行を promote しようとする
        db.set_pdf_meta_thumb("foo.pdf", 2000, 3000, 20).unwrap();

        // 新 mtime/size での lookup は cache miss (stale 行は古いまま、新値で promote されない)
        let new_lookup = db.get_pdf_meta("foo.pdf", 2000, 3000).unwrap();
        assert_eq!(
            new_lookup, None,
            "新 mtime/size の cache hit が起こらない (stale 行 promote されず)"
        );
        // 旧 mtime/size の行はそのまま (page_count=10, password_required=false)
        let old_lookup = db.get_pdf_meta("foo.pdf", 1000, 2000).unwrap();
        assert_eq!(
            old_lookup,
            Some((10, false)),
            "旧 mtime/size の行は元のまま変更されない"
        );
    }

    #[test]
    fn pdf_meta_thumb_updates_page_count_when_mtime_size_match() {
        // mtime/file_size が既存行と一致するときは page_count を更新する (verify update)。
        // password_required 列は保持。
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, true).unwrap();
        // 同じ mtime/size で page_count だけ違う update (例: 別経路でカウントを再計測)
        db.set_pdf_meta_thumb("foo.pdf", 1000, 2048, 35).unwrap();

        let result = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(
            result,
            Some((35, true)),
            "page_count 更新、password_required は保持"
        );
    }

    #[test]
    fn pdf_meta_thumb_noop_when_only_mtime_differs() {
        // **Codex P3 round 4 対応**: mtime だけが異なる stale 状況でも promote しない。
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, true).unwrap();
        // 新 mtime=2000 (size 同じ) で update が来た → 一致条件外
        db.set_pdf_meta_thumb("foo.pdf", 2000, 2048, 50).unwrap();

        // 新 mtime での lookup は cache miss (stale 行は古いまま)
        let new_lookup = db.get_pdf_meta("foo.pdf", 2000, 2048).unwrap();
        assert_eq!(new_lookup, None);
        // 旧 mtime での行は不変
        let old_lookup = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(old_lookup, Some((32, true)));
    }

    #[test]
    fn pdf_meta_thumb_noop_when_only_file_size_differs() {
        // **Codex P3 round 4 対応**: file_size だけが異なる stale 状況でも promote しない。
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, true).unwrap();
        // 新 size=4096 (mtime 同じ) で update が来た → 一致条件外
        db.set_pdf_meta_thumb("foo.pdf", 1000, 4096, 50).unwrap();

        // 新 size での lookup は cache miss
        let new_lookup = db.get_pdf_meta("foo.pdf", 1000, 4096).unwrap();
        assert_eq!(new_lookup, None);
        // 旧 size での行は不変
        let old_lookup = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(old_lookup, Some((32, true)));
    }

    #[test]
    fn pdf_meta_safe_inserts_new_row_with_false() {
        // password=None 確定経路: 新規行を password_required=false で挿入する
        let db = open_in_memory();
        db.set_pdf_meta_safe("new.pdf", 500, 1024, 16).unwrap();

        let result = db.get_pdf_meta("new.pdf", 500, 1024).unwrap();
        assert_eq!(result, Some((16, false)), "新規行は false で挿入");
    }

    #[test]
    fn pdf_meta_safe_overrides_password_required_on_existing_row() {
        // **review #1 対応**: 既存行に password_required=true があっても、safe 経路
        // (= None password で render 成功 = 非保護確信あり) は上書きで 0 にする。
        // 保護版を非保護版に同名差し替えしたとき、stale な「保護」フラグを
        // 永続化しないため。
        let db = open_in_memory();
        db.set_pdf_meta("locked.pdf", 100, 500, 8, true).unwrap();
        db.set_pdf_meta_safe("locked.pdf", 200, 600, 10).unwrap();

        let result = db.get_pdf_meta("locked.pdf", 200, 600).unwrap();
        assert_eq!(
            result,
            Some((10, false)),
            "safe 経路の確信 (password_required=false) で上書きされる"
        );
    }
}
