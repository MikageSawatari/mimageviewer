//! フルスクリーン表示モードの永続管理。
//!
//! `%APPDATA%/mimageviewer/spread.db` にフォルダごとの表示モードを保存する。
//! `rotation_db.rs` と同パターンの SQLite 永続化。

use std::path::{Path, PathBuf};

use crate::path_key;
use crate::settings::{ReadingDirection, ReadingFlow, SpreadMode};
use rusqlite::{OpenFlags, OptionalExtension};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct StoredSpreadState {
    pub(crate) mode: Option<SpreadMode>,
    pub(crate) flow: Option<ReadingFlow>,
    pub(crate) direction: Option<ReadingDirection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpreadContainerKey {
    pub(crate) exact: PathBuf,
    pub(crate) fallback: Option<PathBuf>,
}

/// App と remote IPC が共有する spread.db のコンテナキー規則。
pub(crate) fn container_key(root: &Path, zip_segments: &[String]) -> PathBuf {
    let mut key = root.to_path_buf();
    for segment in zip_segments {
        key.push(segment);
    }
    key
}

pub(crate) fn container_key_with_fallback(
    root: &Path,
    zip_segments: &[String],
) -> SpreadContainerKey {
    SpreadContainerKey {
        exact: container_key(root, zip_segments),
        fallback: (!zip_segments.is_empty()).then(|| root.to_path_buf()),
    }
}

/// 表示モード DB ハンドル
pub struct SpreadDb {
    conn: rusqlite::Connection,
}

impl SpreadDb {
    /// DB を開く (なければ作成)
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        Self::open_at(&path)
    }

    /// 任意の data directory 配下で使うため、DB ファイルを明示して開く。
    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS spreads (
                path TEXT PRIMARY KEY,
                mode INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        ensure_column(&conn, "flow", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_column(&conn, "direction", "INTEGER NOT NULL DEFAULT 0")?;
        Ok(Self { conn })
    }

    /// 既存 DB を read-only で開く。remote IPC の表示設定参照用で、schema 作成や更新は行わない。
    pub fn open_existing_read_only_at(path: &Path) -> Result<Option<Self>, rusqlite::Error> {
        if !path.try_exists().unwrap_or(false) {
            return Ok(None);
        }
        let conn = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Some(Self { conn }))
    }

    /// DB ファイルのパス
    fn db_path() -> PathBuf {
        crate::data_dir::get().join("spread.db")
    }

    /// フォルダの表示モードを取得。未登録なら None。
    pub fn get(&self, path: &Path) -> Option<SpreadMode> {
        let key = normalize_path(path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT mode FROM spreads WHERE path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| {
            let v: i32 = row.get(0)?;
            Ok(SpreadMode::from_int(v))
        })
        .ok()
    }

    /// フォルダの連結方式を取得。旧 `mode=5` は縦連結として扱う。
    pub fn get_flow(&self, path: &Path) -> Option<ReadingFlow> {
        let key = normalize_path(path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT mode, flow FROM spreads WHERE path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| {
            let mode: i32 = row.get(0)?;
            let flow: i32 = row.get(1)?;
            if mode == SpreadMode::Vertical.to_int() && flow == ReadingFlow::Paged.to_int() {
                Ok(ReadingFlow::Vertical)
            } else {
                Ok(ReadingFlow::from_int(flow))
            }
        })
        .ok()
    }

    /// フォルダの横連結方向を取得。未登録なら None。
    pub fn get_direction(&self, path: &Path) -> Option<ReadingDirection> {
        let key = normalize_path(path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT mode, direction FROM spreads WHERE path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| {
            let mode: i32 = row.get(0)?;
            let direction: i32 = row.get(1)?;
            let mode = SpreadMode::from_int(mode);
            if mode.is_rtl() {
                Ok(ReadingDirection::Rtl)
            } else if matches!(mode, SpreadMode::Ltr | SpreadMode::LtrCover) {
                Ok(ReadingDirection::Ltr)
            } else {
                Ok(ReadingDirection::from_int(direction))
            }
        })
        .ok()
    }

    /// 各列を exact key、fallback key の順に解決する。
    /// 旧 ZIP root 行の一部だけを内側の本へ継承する App の規則を remote IPC も共有する。
    pub(crate) fn get_state_with_fallback(
        &self,
        key: &Path,
        fallback: Option<&Path>,
    ) -> StoredSpreadState {
        StoredSpreadState {
            mode: self
                .get(key)
                .or_else(|| fallback.and_then(|fallback| self.get(fallback))),
            flow: self
                .get_flow(key)
                .or_else(|| fallback.and_then(|fallback| self.get_flow(fallback))),
            direction: self
                .get_direction(key)
                .or_else(|| fallback.and_then(|fallback| self.get_direction(fallback))),
        }
    }

    pub fn set_mode_and_direction(
        &mut self,
        path: &Path,
        fallback: Option<&Path>,
        mode: SpreadMode,
        direction: ReadingDirection,
        defaults: (SpreadMode, ReadingFlow, ReadingDirection),
    ) -> Result<(), rusqlite::Error> {
        persist_explicit_spread(&mut self.conn, path, fallback, mode, direction, defaults)
    }

    /// 表示モードを設定する。デフォルト値と同じ場合はレコードを削除する。
    pub fn set(
        &self,
        path: &Path,
        mode: SpreadMode,
        default: SpreadMode,
        default_flow: ReadingFlow,
        default_direction: ReadingDirection,
    ) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        let (flow, direction) = self
            .conn
            .query_row(
                "SELECT flow, direction FROM spreads WHERE path = ?1",
                [&key],
                |row| Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?)),
            )
            .unwrap_or((ReadingFlow::Paged.to_int(), ReadingDirection::Ltr.to_int()));
        if mode == default
            && flow == default_flow.to_int()
            && direction == default_direction.to_int()
        {
            self.conn
                .execute("DELETE FROM spreads WHERE path = ?1", [&key])?;
        } else {
            self.conn.execute(
                "INSERT INTO spreads (path, mode) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET mode = ?2",
                rusqlite::params![key, mode.to_int()],
            )?;
        }
        Ok(())
    }

    /// 連結方式を設定する。ページ構成は維持する。
    pub fn set_flow(
        &self,
        path: &Path,
        flow: ReadingFlow,
        direction: ReadingDirection,
        default_mode: SpreadMode,
        default_flow: ReadingFlow,
        default_direction: ReadingDirection,
    ) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        let mode = self
            .conn
            .query_row("SELECT mode FROM spreads WHERE path = ?1", [&key], |row| {
                row.get::<_, i32>(0)
            })
            .unwrap_or(default_mode.to_int());
        if mode == default_mode.to_int() && flow == default_flow && direction == default_direction {
            self.conn
                .execute("DELETE FROM spreads WHERE path = ?1", [&key])?;
        } else {
            self.conn.execute(
                "INSERT INTO spreads (path, mode, flow, direction) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET flow = ?3, direction = ?4",
                rusqlite::params![key, mode, flow.to_int(), direction.to_int()],
            )?;
        }
        Ok(())
    }

    /// 全レコードを削除 (リセット)
    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM spreads", [])
    }

    /// 登録件数
    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM spreads", [], |row| row.get(0))
            .unwrap_or(0)
    }
}

fn persist_explicit_spread(
    conn: &mut rusqlite::Connection,
    path: &Path,
    fallback: Option<&Path>,
    mode: SpreadMode,
    direction: ReadingDirection,
    defaults: (SpreadMode, ReadingFlow, ReadingDirection),
) -> Result<(), rusqlite::Error> {
    let key = normalize_path(path);
    let fallback = fallback.map(normalize_path);
    let transaction = conn.transaction()?;
    let flow = match stored_flow(&transaction, &key)? {
        Some(flow) => Some(flow),
        None => match fallback.as_deref() {
            Some(fallback) => stored_flow(&transaction, fallback)?,
            None => None,
        },
    }
    .unwrap_or(defaults.1);
    persist_explicit_spread_row(
        &transaction,
        &key,
        mode,
        flow,
        direction,
        defaults,
        fallback.is_none(),
    )?;
    transaction.commit()
}

fn stored_flow(
    transaction: &rusqlite::Transaction<'_>,
    key: &str,
) -> Result<Option<ReadingFlow>, rusqlite::Error> {
    let value = transaction
        .query_row("SELECT flow FROM spreads WHERE path = ?1", [key], |row| {
            row.get::<_, i32>(0)
        })
        .optional()?;
    Ok(value.map(ReadingFlow::from_int))
}

fn persist_explicit_spread_row(
    transaction: &rusqlite::Transaction<'_>,
    key: &str,
    mode: SpreadMode,
    flow: ReadingFlow,
    direction: ReadingDirection,
    defaults: (SpreadMode, ReadingFlow, ReadingDirection),
    delete_if_defaults: bool,
) -> Result<(), rusqlite::Error> {
    if delete_if_defaults && (mode, flow, direction) == defaults {
        transaction.execute("DELETE FROM spreads WHERE path = ?1", [key])?;
    } else {
        transaction.execute(
            "INSERT INTO spreads (path, mode, flow, direction) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET mode = ?2, flow = ?3, direction = ?4",
            rusqlite::params![key, mode.to_int(), flow.to_int(), direction.to_int()],
        )?;
    }
    Ok(())
}

fn normalize_path(path: &Path) -> String {
    path_key::normalize(path)
}

fn ensure_column(
    conn: &rusqlite::Connection,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    let exists = {
        let mut stmt = conn.prepare("PRAGMA table_info(spreads)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut found = false;
        for name in rows {
            if name? == column {
                found = true;
                break;
            }
        }
        found
    };
    if exists {
        return Ok(());
    }
    // PRAGMA→ALTER は非アトミックなので、複数接続が同時に open した場合 (テスト並列実行や
    // 同一 data_dir を指す installed/portable 同時起動) は両方が「列なし」と判定して ALTER
    // し得る。後勝ちの "duplicate column" は目的 (列の存在) が既に達成されているので冪等に
    // 握りつぶす。それ以外のエラーは伝播させる。archive_cache.rs の ADD COLUMN と同じ方針。
    match conn.execute(
        &format!("ALTER TABLE spreads ADD COLUMN {column} {definition}"),
        [],
    ) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(msg))) if msg.contains("duplicate column") => {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_mode_roundtrip() {
        for mode in SpreadMode::all() {
            assert_eq!(SpreadMode::from_int(mode.to_int()), *mode);
        }
        for flow in ReadingFlow::all() {
            assert_eq!(ReadingFlow::from_int(flow.to_int()), *flow);
        }
        assert_eq!(ReadingFlow::Paged.next(), ReadingFlow::Vertical);
        assert_eq!(ReadingFlow::Vertical.next(), ReadingFlow::Horizontal);
        assert_eq!(ReadingFlow::Horizontal.next(), ReadingFlow::Paged);
    }

    #[test]
    fn db_set_get_clear() {
        // 実体の data_dir/spread.db を触らず専用 temp に隔離する。ガードはグローバル
        // ロックを保持するので、open() の PRAGMA→ALTER マイグレーションが他テストと
        // 並列衝突して "duplicate column" で落ちることもない。
        let _guard = crate::data_dir::TestDataDirGuard::new();
        let db = SpreadDb::open().unwrap();
        let p = Path::new("C:/test/folder");
        let default = SpreadMode::Single;

        // 初期状態: 未登録
        assert!(db.get(p).is_none());

        // 設定
        db.set(
            p,
            SpreadMode::Ltr,
            default,
            ReadingFlow::Paged,
            ReadingDirection::Ltr,
        )
        .unwrap();
        assert_eq!(db.get(p), Some(SpreadMode::Ltr));

        // 上書き
        db.set(
            p,
            SpreadMode::RtlCover,
            default,
            ReadingFlow::Paged,
            ReadingDirection::Ltr,
        )
        .unwrap();
        assert_eq!(db.get(p), Some(SpreadMode::RtlCover));

        // 連結方式は表示モードと独立して保存される
        db.set_flow(
            p,
            ReadingFlow::Horizontal,
            ReadingDirection::Rtl,
            SpreadMode::Single,
            ReadingFlow::Paged,
            ReadingDirection::Ltr,
        )
        .unwrap();
        assert_eq!(db.get(p), Some(SpreadMode::RtlCover));
        assert_eq!(db.get_flow(p), Some(ReadingFlow::Horizontal));
        assert_eq!(db.get_direction(p), Some(ReadingDirection::Rtl));

        // デフォルト値で削除
        db.set(
            p,
            SpreadMode::Single,
            default,
            ReadingFlow::Paged,
            ReadingDirection::Ltr,
        )
        .unwrap();
        assert_eq!(db.get_flow(p), Some(ReadingFlow::Horizontal));
    }

    #[test]
    fn legacy_vertical_mode_maps_to_vertical_flow() {
        let _guard = crate::data_dir::TestDataDirGuard::new();
        let db = SpreadDb::open().unwrap();
        let p = Path::new("C:/test/legacy-vertical");

        db.set(
            p,
            SpreadMode::Vertical,
            SpreadMode::Single,
            ReadingFlow::Paged,
            ReadingDirection::Ltr,
        )
        .unwrap();

        assert_eq!(db.get(p), Some(SpreadMode::Vertical));
        assert_eq!(db.get_flow(p), Some(ReadingFlow::Vertical));
        assert_eq!(db.get_direction(p), Some(ReadingDirection::Ltr));
    }

    #[test]
    fn shared_container_key_and_fallback_match_app_rules() {
        let root = Path::new("C:/books/outer.zip");
        let segments = vec!["wrapper".to_owned(), "inner.zip".to_owned()];
        assert_eq!(
            container_key(root, &segments),
            PathBuf::from("C:/books/outer.zip/wrapper/inner.zip")
        );

        let temp = tempfile::tempdir().unwrap();
        let mut db = SpreadDb::open_at(&temp.path().join("spread.db")).unwrap();
        db.set_mode_and_direction(
            root,
            None,
            SpreadMode::RtlCover,
            ReadingDirection::Rtl,
            (
                SpreadMode::Single,
                ReadingFlow::Paged,
                ReadingDirection::Ltr,
            ),
        )
        .unwrap();
        db.set_flow(
            root,
            ReadingFlow::Horizontal,
            ReadingDirection::Rtl,
            SpreadMode::Single,
            ReadingFlow::Paged,
            ReadingDirection::Ltr,
        )
        .unwrap();
        let key = container_key(root, &segments);
        assert_eq!(
            db.get_state_with_fallback(&key, Some(root)),
            StoredSpreadState {
                mode: Some(SpreadMode::RtlCover),
                flow: Some(ReadingFlow::Horizontal),
                direction: Some(ReadingDirection::Rtl),
            }
        );
        db.set_mode_and_direction(
            &key,
            Some(root),
            SpreadMode::Single,
            ReadingDirection::Ltr,
            (
                SpreadMode::Single,
                ReadingFlow::Paged,
                ReadingDirection::Ltr,
            ),
        )
        .unwrap();
        assert_eq!(db.get(&key), Some(SpreadMode::Single));
        assert_eq!(db.get_flow(&key), Some(ReadingFlow::Horizontal));
        assert_eq!(db.get_direction(&key), Some(ReadingDirection::Ltr));
    }

    #[test]
    fn explicit_remote_spread_write_is_atomic_and_preserves_flow() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = SpreadDb::open_at(&temp.path().join("spread.db")).unwrap();
        let book = Path::new("C:/books/book.zip");
        let defaults = (
            SpreadMode::Single,
            ReadingFlow::Paged,
            ReadingDirection::Ltr,
        );
        db.set_flow(
            book,
            ReadingFlow::Horizontal,
            ReadingDirection::Rtl,
            defaults.0,
            defaults.1,
            defaults.2,
        )
        .unwrap();

        db.set_mode_and_direction(
            book,
            None,
            SpreadMode::LtrCover,
            ReadingDirection::Ltr,
            defaults,
        )
        .unwrap();
        assert_eq!(db.get(book), Some(SpreadMode::LtrCover));
        assert_eq!(db.get_flow(book), Some(ReadingFlow::Horizontal));
        assert_eq!(db.get_direction(book), Some(ReadingDirection::Ltr));
    }
}
