//! フルスクリーン表示モードの永続管理。
//!
//! `%APPDATA%/mimageviewer/spread.db` にフォルダごとの表示モードを保存する。
//! `rotation_db.rs` と同パターンの SQLite 永続化。

use std::path::{Path, PathBuf};

use crate::path_key;
use crate::settings::{
    FinalCoverSpreadPreference, PageAlonePreference, PageAlonePreferences, ReadingDirection,
    ReadingFlow, SingletonSpreadEndpointPreferences, SingletonSpreadPlacementPreference,
    SpreadMode,
};
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

    /// App startup may keep reading released preferences when a writable
    /// migration fails. The read-only connection projects the old rows until
    /// the completed marker exists and rejects all preference writes.
    pub(crate) fn open_for_app() -> Result<Self, rusqlite::Error> {
        Self::open_for_app_at(&Self::db_path())
    }

    pub(crate) fn open_for_app_at(path: &Path) -> Result<Self, rusqlite::Error> {
        match Self::open_at(path) {
            Ok(db) => Ok(db),
            Err(migration_error) => {
                let Some(db) = Self::open_existing_read_only_at(path)? else {
                    return Err(migration_error);
                };
                if singleton_endpoint_placement_marker_present(&db.conn)?
                    || !table_exists_checked(&db.conn, "singleton_spread_placements")?
                {
                    return Err(migration_error);
                }
                crate::logger::log(format!(
                    "spread.db migration failed; using read-only released endpoint rows: {migration_error}"
                ));
                Ok(db)
            }
        }
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
            );
            CREATE TABLE IF NOT EXISTS final_cover_spreads (
                path TEXT PRIMARY KEY,
                preference INTEGER NOT NULL CHECK (preference BETWEEN 0 AND 2)
            );
            CREATE TABLE IF NOT EXISTS singleton_spread_placements (
                path TEXT PRIMARY KEY,
                preference INTEGER NOT NULL CHECK (preference BETWEEN 0 AND 2)
            );
            CREATE TABLE IF NOT EXISTS page_alone_preferences (
                path TEXT PRIMARY KEY,
                after_cover_preference INTEGER NOT NULL CHECK (after_cover_preference BETWEEN 0 AND 2),
                last_preference INTEGER NOT NULL CHECK (last_preference BETWEEN 0 AND 2)
            )",
        )?;
        ensure_column(&conn, "flow", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_column(&conn, "direction", "INTEGER NOT NULL DEFAULT 0")?;
        migrate_singleton_endpoint_placements(&conn)?;
        Ok(Self { conn })
    }

    /// 既存 DB を read-only で開く。remote IPC の表示設定参照用で、schema 作成や更新は行わない。
    pub fn open_existing_read_only_at(path: &Path) -> Result<Option<Self>, rusqlite::Error> {
        match path.try_exists() {
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(error) => return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
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

    pub(crate) fn get_final_cover_spread_preference(
        &self,
        path: &Path,
    ) -> Option<FinalCoverSpreadPreference> {
        let key = normalize_path(path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT preference FROM final_cover_spreads WHERE path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| row.get::<_, i32>(0))
            .ok()
            .and_then(FinalCoverSpreadPreference::from_int)
    }

    /// Resolve the exact book preference before its optional container fallback.
    /// An exact `FollowGlobal` is a real value and deliberately blocks fallback.
    /// Read-only handles for old databases have no `final_cover_spreads` table;
    /// the failed lookup is treated as an inherited setting without migrating it.
    pub(crate) fn get_final_cover_spread_preference_with_fallback(
        &self,
        key: &Path,
        fallback: Option<&Path>,
    ) -> FinalCoverSpreadPreference {
        self.get_final_cover_spread_preference(key)
            .or_else(|| {
                fallback.and_then(|fallback| self.get_final_cover_spread_preference(fallback))
            })
            .unwrap_or_default()
    }

    /// Store a final-cover override without creating or modifying the legacy
    /// `spreads` row. At a root key, following the global value needs no row;
    /// at a nested key it is stored explicitly so that it blocks root fallback.
    pub(crate) fn set_final_cover_spread_preference(
        &self,
        path: &Path,
        fallback: Option<&Path>,
        preference: FinalCoverSpreadPreference,
    ) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        if preference == FinalCoverSpreadPreference::FollowGlobal && fallback.is_none() {
            self.conn
                .execute("DELETE FROM final_cover_spreads WHERE path = ?1", [&key])?;
        } else {
            self.conn.execute(
                "INSERT INTO final_cover_spreads (path, preference) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET preference = ?2",
                rusqlite::params![key, preference.to_int()],
            )?;
        }
        Ok(())
    }

    /// Marker-gated endpoint read. A read-only released DB projects each old
    /// row to both endpoints; an interrupted new table cannot mask old rows.
    pub(crate) fn get_singleton_spread_endpoint_preferences_with_fallback(
        &self,
        key: &Path,
        fallback: Option<&Path>,
    ) -> Result<SingletonSpreadEndpointPreferences, rusqlite::Error> {
        let migrated = singleton_endpoint_placement_marker_present(&self.conn)?;
        let read =
            |path: &Path| -> Result<Option<SingletonSpreadEndpointPreferences>, rusqlite::Error> {
                if migrated {
                    let key = normalize_path(path);
                    let values: Option<(i32, i32)> = self.conn.query_row(
                    "SELECT first_preference, last_preference FROM singleton_spread_endpoint_placements WHERE path = ?1",
                    [&key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ).optional()?;
                    values
                        .map(|(first, last)| {
                            Ok(SingletonSpreadEndpointPreferences {
                                first: SingletonSpreadPlacementPreference::from_int(first)
                                    .ok_or(rusqlite::Error::InvalidQuery)?,
                                last: SingletonSpreadPlacementPreference::from_int(last)
                                    .ok_or(rusqlite::Error::InvalidQuery)?,
                            })
                        })
                        .transpose()
                } else if table_exists_checked(&self.conn, "singleton_spread_placements")? {
                    let key = normalize_path(path);
                    let old: Option<i32> = self
                        .conn
                        .query_row(
                            "SELECT preference FROM singleton_spread_placements WHERE path = ?1",
                            [&key],
                            |row| row.get(0),
                        )
                        .optional()?;
                    old.map(|value| {
                        let preference = SingletonSpreadPlacementPreference::from_int(value)
                            .ok_or(rusqlite::Error::InvalidQuery)?;
                        Ok(SingletonSpreadEndpointPreferences {
                            first: preference,
                            last: preference,
                        })
                    })
                    .transpose()
                } else {
                    Ok(None)
                }
            };
        if migrated && !table_exists_checked(&self.conn, "singleton_spread_endpoint_placements")? {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if let Some(preferences) = read(key)? {
            return Ok(preferences);
        }
        if let Some(fallback) = fallback {
            if let Some(preferences) = read(fallback)? {
                return Ok(preferences);
            }
        }
        Ok(SingletonSpreadEndpointPreferences::default())
    }

    pub(crate) fn set_singleton_spread_endpoint_preferences(
        &self,
        path: &Path,
        fallback: Option<&Path>,
        preferences: SingletonSpreadEndpointPreferences,
    ) -> Result<(), rusqlite::Error> {
        if !singleton_endpoint_placement_marker_present(&self.conn)? {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let key = normalize_path(path);
        if preferences == SingletonSpreadEndpointPreferences::default() && fallback.is_none() {
            self.conn.execute(
                "DELETE FROM singleton_spread_endpoint_placements WHERE path = ?1",
                [&key],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO singleton_spread_endpoint_placements (path, first_preference, last_preference) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO UPDATE SET first_preference = ?2, last_preference = ?3",
                rusqlite::params![key, preferences.first.to_int(), preferences.last.to_int()],
            )?;
        }
        Ok(())
    }

    /// An absent table in a released read-only database means both controls
    /// inherit their new default-OFF global settings. A damaged present table
    /// remains an error rather than being mistaken for inherited preferences.
    pub(crate) fn get_page_alone_preferences_with_fallback(
        &self,
        key: &Path,
        fallback: Option<&Path>,
    ) -> Result<PageAlonePreferences, rusqlite::Error> {
        if !table_exists_checked(&self.conn, "page_alone_preferences")? {
            return Ok(PageAlonePreferences::default());
        }
        let read = |path: &Path| -> Result<Option<PageAlonePreferences>, rusqlite::Error> {
            let key = normalize_path(path);
            self.conn.query_row(
                "SELECT after_cover_preference, last_preference FROM page_alone_preferences WHERE path = ?1",
                [&key],
                |row| {
                    let after_cover = PageAlonePreference::from_int(row.get(0)?)
                        .ok_or(rusqlite::Error::InvalidQuery)?;
                    let last = PageAlonePreference::from_int(row.get(1)?)
                        .ok_or(rusqlite::Error::InvalidQuery)?;
                    Ok(PageAlonePreferences { after_cover, last })
                },
            ).optional()
        };
        if let Some(preferences) = read(key)? {
            return Ok(preferences);
        }
        if let Some(fallback) = fallback {
            if let Some(preferences) = read(fallback)? {
                return Ok(preferences);
            }
        }
        Ok(PageAlonePreferences::default())
    }

    pub(crate) fn set_page_alone_preferences(
        &self,
        path: &Path,
        fallback: Option<&Path>,
        preferences: PageAlonePreferences,
    ) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        if preferences == PageAlonePreferences::default() && fallback.is_none() {
            self.conn
                .execute("DELETE FROM page_alone_preferences WHERE path = ?1", [&key])?;
        } else {
            self.conn.execute(
                "INSERT INTO page_alone_preferences (path, after_cover_preference, last_preference) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO UPDATE SET after_cover_preference = ?2, last_preference = ?3",
                rusqlite::params![key, preferences.after_cover.to_int(), preferences.last.to_int()],
            )?;
        }
        Ok(())
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

    /// Persist the displayed mode, flow, and direction as one SQLite statement.
    /// UI setters use this before changing any in-memory presentation state.
    pub(crate) fn set_presentation_state(
        &self,
        path: &Path,
        mode: SpreadMode,
        flow: ReadingFlow,
        direction: ReadingDirection,
        defaults: (SpreadMode, ReadingFlow, ReadingDirection),
    ) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        if (mode, flow, direction) == defaults {
            self.conn
                .execute("DELETE FROM spreads WHERE path = ?1", [&key])?;
        } else {
            self.conn.execute(
                "INSERT INTO spreads (path, mode, flow, direction) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET mode = ?2, flow = ?3, direction = ?4",
                rusqlite::params![key, mode.to_int(), flow.to_int(), direction.to_int()],
            )?;
        }
        Ok(())
    }

    /// 全レコードを削除 (リセット)
    pub fn clear_all(&mut self) -> Result<usize, rusqlite::Error> {
        let transaction = self.conn.transaction()?;
        let spreads = transaction.execute("DELETE FROM spreads", [])?;
        let final_covers = transaction.execute("DELETE FROM final_cover_spreads", [])?;
        let singleton_placements =
            transaction.execute("DELETE FROM singleton_spread_endpoint_placements", [])?;
        let page_alone = transaction.execute("DELETE FROM page_alone_preferences", [])?;
        transaction.execute("DELETE FROM singleton_spread_placements", [])?;
        transaction.commit()?;
        Ok(spreads + final_covers + singleton_placements + page_alone)
    }

    /// 登録件数
    pub fn count(&self) -> usize {
        let mut tables = vec!["spreads"];
        if table_exists(&self.conn, "final_cover_spreads") {
            tables.push("final_cover_spreads");
        }
        let endpoint_table =
            if singleton_endpoint_placement_marker_present(&self.conn).unwrap_or(false) {
                "singleton_spread_endpoint_placements"
            } else {
                "singleton_spread_placements"
            };
        if table_exists(&self.conn, endpoint_table) {
            tables.push(endpoint_table);
        }
        if table_exists(&self.conn, "page_alone_preferences") {
            tables.push("page_alone_preferences");
        }
        let union = tables
            .iter()
            .map(|table| format!("SELECT path FROM {table}"))
            .collect::<Vec<_>>()
            .join(" UNION ");
        self.conn
            .query_row(&format!("SELECT COUNT(*) FROM ({union})"), [], |row| {
                row.get::<_, usize>(0)
            })
            .unwrap_or(0)
    }
}

fn table_exists(conn: &rusqlite::Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1
         )",
        [table],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

fn table_exists_checked(conn: &rusqlite::Connection, table: &str) -> Result<bool, rusqlite::Error> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false))
}

fn singleton_endpoint_placement_marker_present(
    conn: &rusqlite::Connection,
) -> Result<bool, rusqlite::Error> {
    if !table_exists_checked(conn, "spread_meta")? {
        return Ok(false);
    }
    Ok(conn
        .query_row(
            "SELECT 1 FROM spread_meta WHERE key = 'singleton_endpoint_placements_v1' AND value = '1'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false))
}

fn migrate_singleton_endpoint_placements(
    conn: &rusqlite::Connection,
) -> Result<(), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS spread_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS singleton_spread_endpoint_placements (
             path TEXT PRIMARY KEY,
             first_preference INTEGER NOT NULL CHECK (first_preference BETWEEN 0 AND 2),
             last_preference INTEGER NOT NULL CHECK (last_preference BETWEEN 0 AND 2)
         );",
    )?;
    if !singleton_endpoint_placement_marker_present(&tx)? {
        // A table left by an interrupted attempt is never authoritative.
        tx.execute("DELETE FROM singleton_spread_endpoint_placements", [])?;
        tx.execute(
            "INSERT INTO singleton_spread_endpoint_placements (path, first_preference, last_preference)
             SELECT path, preference, preference FROM singleton_spread_placements",
            [],
        )?;
        tx.execute(
            "INSERT INTO spread_meta(key, value) VALUES ('singleton_endpoint_placements_v1', '1')",
            [],
        )?;
    }
    tx.commit()?;
    Ok(())
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

    #[test]
    fn endpoint_preferences_project_read_only_legacy_and_retry_interrupted_migration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("spread.db");
        let root = Path::new("C:/books/outer.zip");
        let nested = Path::new("C:/books/outer.zip/book");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE singleton_spread_placements (path TEXT PRIMARY KEY, preference INTEGER NOT NULL);
                 CREATE TABLE singleton_spread_endpoint_placements (path TEXT PRIMARY KEY, first_preference INTEGER, last_preference INTEGER);",
            ).unwrap();
            conn.execute(
                "INSERT INTO singleton_spread_placements(path, preference) VALUES (?1, 1)",
                [normalize_path(root)],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO singleton_spread_placements(path, preference) VALUES (?1, 0)",
                [normalize_path(nested)],
            )
            .unwrap();
            conn.execute("INSERT INTO singleton_spread_endpoint_placements(path, first_preference, last_preference) VALUES (?1, 2, 2)", [normalize_path(root)]).unwrap();
        }
        let read_only = SpreadDb::open_existing_read_only_at(&path)
            .unwrap()
            .unwrap();
        assert_eq!(
            read_only
                .get_singleton_spread_endpoint_preferences_with_fallback(root, None)
                .unwrap(),
            SingletonSpreadEndpointPreferences::from(SingletonSpreadPlacementPreference::Place)
        );
        assert_eq!(
            read_only
                .get_singleton_spread_endpoint_preferences_with_fallback(nested, Some(root))
                .unwrap(),
            SingletonSpreadEndpointPreferences::default(),
            "explicit nested FollowGlobal blocks root fallback"
        );
        drop(read_only);
        let db = SpreadDb::open_at(&path).unwrap();
        assert!(singleton_endpoint_placement_marker_present(&db.conn).unwrap());
        assert_eq!(
            db.get_singleton_spread_endpoint_preferences_with_fallback(root, None)
                .unwrap(),
            SingletonSpreadEndpointPreferences::from(SingletonSpreadPlacementPreference::Place)
        );
        assert_eq!(
            db.get_singleton_spread_endpoint_preferences_with_fallback(nested, Some(root))
                .unwrap(),
            SingletonSpreadEndpointPreferences::default()
        );
        db.conn
            .execute_batch("DROP TABLE singleton_spread_endpoint_placements")
            .unwrap();
        assert!(
            db.get_singleton_spread_endpoint_preferences_with_fallback(root, None)
                .is_err()
        );
    }

    #[test]
    fn endpoint_preferences_are_independent_and_preserve_nested_override() {
        let temp = tempfile::tempdir().unwrap();
        let db = SpreadDb::open_at(&temp.path().join("spread.db")).unwrap();
        let root = Path::new("C:/books/outer.zip");
        let nested = Path::new("C:/books/outer.zip/book");
        db.set_singleton_spread_endpoint_preferences(
            root,
            None,
            SingletonSpreadEndpointPreferences {
                first: SingletonSpreadPlacementPreference::Place,
                last: SingletonSpreadPlacementPreference::Center,
            },
        )
        .unwrap();
        db.set_singleton_spread_endpoint_preferences(
            nested,
            Some(root),
            SingletonSpreadEndpointPreferences {
                first: SingletonSpreadPlacementPreference::FollowGlobal,
                last: SingletonSpreadPlacementPreference::Place,
            },
        )
        .unwrap();
        assert_eq!(
            db.get_singleton_spread_endpoint_preferences_with_fallback(nested, Some(root))
                .unwrap(),
            SingletonSpreadEndpointPreferences {
                first: SingletonSpreadPlacementPreference::FollowGlobal,
                last: SingletonSpreadPlacementPreference::Place
            }
        );
    }

    #[test]
    fn endpoint_migration_failure_keeps_legacy_rows_for_retry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("spread.db");
        let key = Path::new("C:/books/book.zip");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE singleton_spread_placements (path TEXT PRIMARY KEY, preference INTEGER NOT NULL);
                 CREATE TABLE singleton_spread_endpoint_placements (path TEXT PRIMARY KEY, first_preference INTEGER, last_preference INTEGER);
                 CREATE TRIGGER fail_endpoint_copy BEFORE INSERT ON singleton_spread_endpoint_placements
                 BEGIN SELECT RAISE(ABORT, 'injected migration failure'); END;",
            ).unwrap();
            conn.execute(
                "INSERT INTO singleton_spread_placements(path, preference) VALUES (?1, 1)",
                [normalize_path(key)],
            )
            .unwrap();
        }
        assert!(SpreadDb::open_at(&path).is_err());
        let read_only = SpreadDb::open_existing_read_only_at(&path)
            .unwrap()
            .unwrap();
        assert_eq!(
            read_only
                .get_singleton_spread_endpoint_preferences_with_fallback(key, None)
                .unwrap(),
            SingletonSpreadEndpointPreferences::from(SingletonSpreadPlacementPreference::Place)
        );
        drop(read_only);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("DROP TRIGGER fail_endpoint_copy")
            .unwrap();
        let retry = SpreadDb::open_at(&path).unwrap();
        assert_eq!(
            retry
                .get_singleton_spread_endpoint_preferences_with_fallback(key, None)
                .unwrap(),
            SingletonSpreadEndpointPreferences::from(SingletonSpreadPlacementPreference::Place)
        );
    }

    #[test]
    fn final_cover_override_is_independent_and_exact_follow_global_blocks_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = SpreadDb::open_at(&temp.path().join("spread.db")).unwrap();
        let root = Path::new("C:/books/outer.zip");
        let nested = Path::new("C:/books/outer.zip/book");

        db.set(
            root,
            SpreadMode::RtlCover,
            SpreadMode::Single,
            ReadingFlow::Paged,
            ReadingDirection::Ltr,
        )
        .unwrap();
        db.set_final_cover_spread_preference(root, None, FinalCoverSpreadPreference::Off)
            .unwrap();
        assert_eq!(
            db.get_final_cover_spread_preference_with_fallback(nested, Some(root)),
            FinalCoverSpreadPreference::Off
        );

        db.set_final_cover_spread_preference(
            nested,
            Some(root),
            FinalCoverSpreadPreference::FollowGlobal,
        )
        .unwrap();
        assert_eq!(
            db.get_final_cover_spread_preference_with_fallback(nested, Some(root)),
            FinalCoverSpreadPreference::FollowGlobal
        );
        assert_eq!(db.get(root), Some(SpreadMode::RtlCover));

        db.set_final_cover_spread_preference(nested, Some(root), FinalCoverSpreadPreference::On)
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
        assert_eq!(
            db.get_final_cover_spread_preference_with_fallback(nested, Some(root)),
            FinalCoverSpreadPreference::On
        );
        assert_eq!(db.get_flow(root), Some(ReadingFlow::Horizontal));
        assert_eq!(db.count(), 2, "paths shared by both tables count once");
        assert_eq!(db.clear_all().unwrap(), 3);
        assert_eq!(db.count(), 0);
        assert!(db.get(root).is_none());
        assert_eq!(
            db.get_final_cover_spread_preference_with_fallback(nested, Some(root)),
            FinalCoverSpreadPreference::FollowGlobal
        );
    }

    #[test]
    fn singleton_placement_override_is_independent_and_exact_follow_global_blocks_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = SpreadDb::open_at(&temp.path().join("spread.db")).unwrap();
        let root = Path::new("C:/books/outer.zip");
        let nested = Path::new("C:/books/outer.zip/book");

        db.set_singleton_spread_endpoint_preferences(
            root,
            None,
            SingletonSpreadPlacementPreference::Center.into(),
        )
        .unwrap();
        assert_eq!(
            db.get_singleton_spread_endpoint_preferences_with_fallback(nested, Some(root))
                .unwrap(),
            SingletonSpreadEndpointPreferences::from(SingletonSpreadPlacementPreference::Center)
        );

        db.set_singleton_spread_endpoint_preferences(
            nested,
            Some(root),
            SingletonSpreadPlacementPreference::FollowGlobal.into(),
        )
        .unwrap();
        assert_eq!(
            db.get_singleton_spread_endpoint_preferences_with_fallback(nested, Some(root))
                .unwrap(),
            SingletonSpreadEndpointPreferences::default()
        );

        db.set_singleton_spread_endpoint_preferences(
            nested,
            Some(root),
            SingletonSpreadPlacementPreference::Place.into(),
        )
        .unwrap();
        assert_eq!(
            db.get_singleton_spread_endpoint_preferences_with_fallback(nested, Some(root))
                .unwrap(),
            SingletonSpreadEndpointPreferences::from(SingletonSpreadPlacementPreference::Place)
        );
        assert_eq!(db.count(), 2, "root and nested keys count once each");
        assert_eq!(db.clear_all().unwrap(), 2);
        assert_eq!(db.count(), 0);
    }

    #[test]
    fn page_alone_preferences_keep_independent_nested_overrides_and_read_only_compatibility() {
        use crate::settings::{PageAlonePreference as Preference, PageAlonePreferences};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("spread.db");
        let root = Path::new("C:/books/outer.zip");
        let nested = Path::new("C:/books/outer.zip/book");
        let db = SpreadDb::open_at(&path).unwrap();
        db.set_page_alone_preferences(
            root,
            None,
            PageAlonePreferences {
                after_cover: Preference::On,
                last: Preference::Off,
            },
        )
        .unwrap();
        assert_eq!(
            db.get_page_alone_preferences_with_fallback(nested, Some(root))
                .unwrap(),
            PageAlonePreferences {
                after_cover: Preference::On,
                last: Preference::Off
            },
        );
        db.set_page_alone_preferences(nested, Some(root), PageAlonePreferences::default())
            .unwrap();
        assert_eq!(
            db.get_page_alone_preferences_with_fallback(nested, Some(root))
                .unwrap(),
            PageAlonePreferences::default(),
            "explicit nested FollowGlobal must block the root row",
        );
        assert_eq!(db.count(), 2);
        db.conn
            .execute_batch(
                "ALTER TABLE page_alone_preferences RENAME TO damaged_page_alone_preferences;",
            )
            .unwrap();
        assert_eq!(
            db.get_page_alone_preferences_with_fallback(root, None)
                .unwrap(),
            PageAlonePreferences::default(),
            "missing table on a read-only released DB inherits defaults",
        );
        let old = SpreadDb::open_existing_read_only_at(&path)
            .unwrap()
            .unwrap();
        assert_eq!(
            old.get_page_alone_preferences_with_fallback(root, None)
                .unwrap(),
            PageAlonePreferences::default(),
        );
        assert!(
            old.set_page_alone_preferences(root, None, PageAlonePreferences::default())
                .is_err()
        );
        db.conn
            .execute_batch(
                "CREATE TABLE page_alone_preferences(path TEXT PRIMARY KEY, wrong INTEGER);",
            )
            .unwrap();
        assert!(
            db.get_page_alone_preferences_with_fallback(root, None)
                .is_err()
        );
    }

    #[test]
    fn old_read_only_schema_inherits_final_cover_without_migration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("spread.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "CREATE TABLE spreads (path TEXT PRIMARY KEY, mode INTEGER NOT NULL DEFAULT 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO spreads (path, mode) VALUES ('c:/books/legacy.zip', 2)",
            [],
        )
        .unwrap();
        drop(conn);

        let db = SpreadDb::open_existing_read_only_at(&path)
            .unwrap()
            .unwrap();
        assert_eq!(
            db.get_final_cover_spread_preference_with_fallback(
                Path::new("C:/books/book.zip"),
                None,
            ),
            FinalCoverSpreadPreference::FollowGlobal
        );
        assert_eq!(db.count(), 1);
        let conn = rusqlite::Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare("PRAGMA table_info(final_cover_spreads)")
            .unwrap();
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.is_empty());
        assert_eq!(
            db.get_singleton_spread_endpoint_preferences_with_fallback(
                Path::new("C:/books/book.zip"),
                None,
            )
            .unwrap(),
            SingletonSpreadEndpointPreferences::default()
        );
        let mut stmt = conn
            .prepare("PRAGMA table_info(singleton_spread_placements)")
            .unwrap();
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.is_empty());
    }

    #[test]
    fn transitional_read_only_schema_counts_each_optional_table_without_duplicates() {
        for optional_table in ["final_cover_spreads", "singleton_spread_placements"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("spread.db");
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "CREATE TABLE spreads (path TEXT PRIMARY KEY, mode INTEGER NOT NULL DEFAULT 0)",
                [],
            )
            .unwrap();
            conn.execute(
                &format!(
                    "CREATE TABLE {optional_table} (
                        path TEXT PRIMARY KEY,
                        preference INTEGER NOT NULL
                    )"
                ),
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO spreads (path, mode) VALUES ('c:/books/shared.zip', 2)",
                [],
            )
            .unwrap();
            conn.execute(
                &format!(
                    "INSERT INTO {optional_table} (path, preference)
                     VALUES ('c:/books/shared.zip', 1), ('c:/books/optional-only.zip', 2)"
                ),
                [],
            )
            .unwrap();
            drop(conn);

            let db = SpreadDb::open_existing_read_only_at(&path)
                .unwrap()
                .unwrap();
            assert_eq!(db.count(), 2, "table={optional_table}");
        }
    }
}
