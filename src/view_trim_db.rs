//! 表示トリムの永続管理。
//!
//! `%APPDATA%/mimageviewer/view_trim.db` に、本ごとの適用モード / 本全体設定と、
//! ページごとの個別トリム設定を保存する。表示専用の設定であり、export crop や
//! 補正 / AI パイプラインの出力には影響しない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::view_trim::{
    ViewTrimApplyMode, ViewTrimBookPreference, ViewTrimBookProvenance, ViewTrimBookSettings,
    ViewTrimBookState, ViewTrimPageOverride, decode_view_trim_book_state,
};
use rusqlite::{OpenFlags, OptionalExtension};

#[derive(Debug)]
pub enum ViewTrimBookLoad {
    Missing,
    Stored(ViewTrimBookState, ViewTrimBookProvenance),
    Invalid(String),
    Error(rusqlite::Error),
}

impl ViewTrimBookLoad {
    /// 失敗は全体既定へ継承せず、保存しない安全な None として表示する。
    pub fn state_or_default(&self) -> ViewTrimBookState {
        match self {
            Self::Missing => ViewTrimBookState::default(),
            Self::Stored(state, _) => *state,
            Self::Invalid(_) | Self::Error(_) => ViewTrimBookState {
                preference: ViewTrimBookPreference::None,
                ..Default::default()
            },
        }
    }

    pub fn allows_legacy_margin_fit_migration(&self) -> bool {
        matches!(
            self,
            Self::Missing
                | Self::Stored(_, ViewTrimBookProvenance::Legacy(ViewTrimApplyMode::None))
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewTrimWriteBatch {
    pub book: Option<(PathBuf, Option<PathBuf>, ViewTrimBookState)>,
    pub pages: Vec<(String, ViewTrimPageOverride)>,
}

pub struct ViewTrimDb {
    conn: rusqlite::Connection,
}

impl ViewTrimDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS view_trim_books (
                book_key      TEXT PRIMARY KEY,
                state_json    TEXT NOT NULL,
                updated_at    INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE TABLE IF NOT EXISTS view_trim_pages (
                page_path     TEXT PRIMARY KEY,
                override_json TEXT NOT NULL,
                updated_at    INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        )?;
        Ok(Self { conn })
    }

    /// 既存 DB を read-only で開く。remote 表示からの参照用で、schema 作成や更新は行わない。
    pub fn open_existing_read_only_at(path: &Path) -> Result<Option<Self>, rusqlite::Error> {
        // An existence-check error is not a missing DB. Let SQLite report the open failure.
        if !path.try_exists().unwrap_or(true) {
            return Ok(None);
        }
        let conn = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Some(Self { conn }))
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("view_trim.db")
    }

    fn load_book_preference(&self, book_path: &Path) -> ViewTrimBookLoad {
        let key = book_key(book_path);
        let result = self
            .conn
            .query_row(
                "SELECT state_json FROM view_trim_books WHERE book_key = ?1",
                [&key],
                |row| row.get::<_, String>(0),
            )
            .optional();
        match result {
            Ok(None) => ViewTrimBookLoad::Missing,
            Ok(Some(json)) => match decode_view_trim_book_state(&json) {
                Ok((state, provenance)) => ViewTrimBookLoad::Stored(state, provenance),
                Err(error) => {
                    crate::logger::log(format!("view trim state invalid: {key}: {error}"));
                    ViewTrimBookLoad::Invalid(error.to_string())
                }
            },
            Err(error) => {
                crate::logger::log(format!("view trim read failed: {key}: {error}"));
                ViewTrimBookLoad::Error(error)
            }
        }
    }

    pub fn load_book_preference_with_fallback(
        &self,
        exact: &Path,
        fallback: Option<&Path>,
    ) -> ViewTrimBookLoad {
        match self.load_book_preference(exact) {
            ViewTrimBookLoad::Missing => fallback
                .map(|path| self.load_book_preference(path))
                .unwrap_or(ViewTrimBookLoad::Missing),
            loaded => loaded,
        }
    }

    pub fn set_book_state(
        &self,
        book_path: &Path,
        fallback: Option<&Path>,
        state: ViewTrimBookState,
    ) -> Result<(), rusqlite::Error> {
        let _page_edit_write = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin();
        write_book_state(&self.conn, book_path, fallback, state)
    }

    pub fn set_page_override(
        &self,
        page_key: &str,
        page_override: ViewTrimPageOverride,
    ) -> Result<(), rusqlite::Error> {
        let _page_edit_write =
            crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin_for_key(page_key);
        let json = serde_json::to_string(&page_override).unwrap_or_else(|_| "{}".to_string());
        self.conn.execute(
            "INSERT INTO view_trim_pages (page_path, override_json, updated_at)
             VALUES (?1, ?2, unixepoch())
             ON CONFLICT(page_path) DO UPDATE SET
                override_json = ?2,
                updated_at = unixepoch()",
            rusqlite::params![page_key, json],
        )?;
        Ok(())
    }

    pub fn get_page_override(&self, page_key: &str) -> Option<ViewTrimPageOverride> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT override_json FROM view_trim_pages WHERE page_path = ?1")
            .ok()?;
        let json: String = stmt.query_row([page_key], |row| row.get(0)).ok()?;
        serde_json::from_str(&json).ok()
    }

    pub fn remove_page_override(&self, page_key: &str) -> Result<(), rusqlite::Error> {
        let _page_edit_write =
            crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin_for_key(page_key);
        self.conn.execute(
            "DELETE FROM view_trim_pages WHERE page_path = ?1",
            [page_key],
        )?;
        Ok(())
    }

    /// 明示メタ情報転送前に UI から引き渡された未保存値を、worker 接続で一括保存する。
    /// 同一 batch は 1 transaction なので、export が途中状態を読むことはない。
    pub fn apply_write_batch(&mut self, batch: &ViewTrimWriteBatch) -> Result<(), rusqlite::Error> {
        let _page_edit_write = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin();
        if batch.book.is_none() && batch.pages.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        if let Some((book_path, fallback, state)) = &batch.book {
            write_book_state(&tx, book_path, fallback.as_deref(), *state)?;
        }
        for (page_key, page_override) in &batch.pages {
            let json = serde_json::to_string(page_override)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            tx.execute(
                "INSERT INTO view_trim_pages (page_path, override_json, updated_at)
                 VALUES (?1, ?2, unixepoch())
                 ON CONFLICT(page_path) DO UPDATE SET
                    override_json = ?2,
                    updated_at = unixepoch()",
                rusqlite::params![page_key, json],
            )?;
        }
        tx.commit()
    }

    pub fn load_page_overrides_by_prefix(
        &self,
        prefix: &str,
    ) -> HashMap<String, ViewTrimPageOverride> {
        let mut map = HashMap::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path, override_json FROM view_trim_pages
             WHERE page_path LIKE ?1 ESCAPE '\\'",
        ) else {
            return map;
        };
        let pattern = format!("{}%", crate::adjustment_db::escape_like_pattern(prefix));
        let Ok(rows) = stmt.query_map([&pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            return map;
        };
        for (key, json) in rows.flatten() {
            if let Ok(page_override) = serde_json::from_str::<ViewTrimPageOverride>(&json) {
                map.insert(key, page_override);
            }
        }
        map
    }

    /// 複数フォルダを横断する一覧向けに、指定キーだけを一括読込する。
    pub fn load_page_overrides_many(
        &self,
        page_keys: &[&str],
    ) -> HashMap<String, ViewTrimPageOverride> {
        let mut map = HashMap::new();
        for chunk in page_keys.chunks(500) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT page_path, override_json FROM view_trim_pages
                 WHERE page_path IN ({placeholders})"
            );
            let Ok(mut stmt) = self.conn.prepare(&sql) else {
                continue;
            };
            let Ok(rows) = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter().copied()), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
            else {
                continue;
            };
            for (key, json) in rows.flatten() {
                if let Ok(page_override) = serde_json::from_str::<ViewTrimPageOverride>(&json) {
                    map.insert(key, page_override);
                }
            }
        }
        map
    }
}

/// 削除可否は fallback を知る保存境界だけで決める。ページ行には触れない。
fn write_book_state(
    conn: &rusqlite::Connection,
    exact: &Path,
    fallback: Option<&Path>,
    state: ViewTrimBookState,
) -> Result<(), rusqlite::Error> {
    let key = book_key(exact);
    write_book_state_row(conn, "view_trim_books", &key, fallback.is_some(), state)
}

/// 明示メタ情報 import の attached transaction も通常保存と同じ削除規則を使う。
pub(crate) fn write_transferred_book_state(
    conn: &rusqlite::Connection,
    key: &str,
    has_fallback: bool,
    state: ViewTrimBookState,
) -> Result<(), rusqlite::Error> {
    write_book_state_row(conn, "view_trim.view_trim_books", key, has_fallback, state)
}

fn write_book_state_row(
    conn: &rusqlite::Connection,
    table: &str,
    key: &str,
    has_fallback: bool,
    state: ViewTrimBookState,
) -> Result<(), rusqlite::Error> {
    if !has_fallback
        && state.preference == ViewTrimBookPreference::FollowGlobal
        && state.book_settings == ViewTrimBookSettings::default()
    {
        conn.execute(&format!("DELETE FROM {table} WHERE book_key = ?1"), [key])?;
    } else {
        let json = serde_json::to_string(&state)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        conn.execute(
            &format!(
                "INSERT INTO {table} (book_key, state_json, updated_at)
             VALUES (?1, ?2, unixepoch())
             ON CONFLICT(book_key) DO UPDATE SET
                state_json = ?2,
                updated_at = unixepoch()"
            ),
            rusqlite::params![key, json],
        )?;
    }
    Ok(())
}

fn book_key(path: &Path) -> String {
    crate::path_key::normalize(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_trim::{
        ViewTrimBookPreference, ViewTrimBookSettings, ViewTrimBookState, ViewTrimMargins,
        ViewTrimPageOverride,
    };

    fn open_temp_db() -> (tempfile::TempDir, ViewTrimDb) {
        let temp = tempfile::tempdir().unwrap();
        let db = ViewTrimDb::open_at(&temp.path().join("view_trim.db")).unwrap();
        (temp, db)
    }

    #[test]
    fn follow_global_nested_marker_blocks_root_and_modes_keep_manual_and_page_values() {
        let (_temp, mut db) = open_temp_db();
        let root = Path::new("C:/books/book.zip");
        let child = Path::new("C:/books/book.zip/child");
        let settings = ViewTrimBookSettings {
            enabled: true,
            single: ViewTrimMargins {
                left: 0.07,
                ..Default::default()
            },
            ..Default::default()
        };
        db.set_book_state(
            root,
            None,
            ViewTrimBookState {
                preference: ViewTrimBookPreference::Auto,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            db.load_book_preference_with_fallback(child, Some(root))
                .state_or_default()
                .preference,
            ViewTrimBookPreference::Auto
        );
        db.set_book_state(child, Some(root), ViewTrimBookState::default())
            .unwrap();
        assert!(matches!(
            db.load_book_preference_with_fallback(child, Some(root)),
            ViewTrimBookLoad::Stored(
                ViewTrimBookState {
                    preference: ViewTrimBookPreference::FollowGlobal,
                    ..
                },
                ViewTrimBookProvenance::Current
            )
        ));
        let page = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            top: 0.06,
            ..Default::default()
        });
        db.set_page_override("page", page).unwrap();
        for preference in [
            ViewTrimBookPreference::None,
            ViewTrimBookPreference::Auto,
            ViewTrimBookPreference::Manual,
            ViewTrimBookPreference::FollowGlobal,
        ] {
            let state = ViewTrimBookState {
                preference,
                book_settings: settings,
            };
            db.apply_write_batch(&ViewTrimWriteBatch {
                book: Some((child.to_owned(), Some(root.to_owned()), state)),
                pages: vec![],
            })
            .unwrap();
            assert_eq!(
                db.load_book_preference_with_fallback(child, Some(root))
                    .state_or_default(),
                state
            );
            assert_eq!(db.get_page_override("page"), Some(page));
            db.set_book_state(
                root,
                None,
                ViewTrimBookState {
                    preference,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                matches!(
                    db.load_book_preference_with_fallback(root, None),
                    ViewTrimBookLoad::Missing
                ),
                preference == ViewTrimBookPreference::FollowGlobal
            );
        }
        db.apply_write_batch(&ViewTrimWriteBatch {
            book: Some((
                child.to_owned(),
                Some(root.to_owned()),
                ViewTrimBookState::default(),
            )),
            pages: vec![],
        })
        .unwrap();
        assert!(matches!(
            db.load_book_preference_with_fallback(child, Some(root)),
            ViewTrimBookLoad::Stored(_, ViewTrimBookProvenance::Current)
        ));
        let retained = ViewTrimBookState {
            preference: ViewTrimBookPreference::FollowGlobal,
            book_settings: settings,
        };
        db.set_book_state(root, None, retained).unwrap();
        assert_eq!(
            db.load_book_preference_with_fallback(root, None)
                .state_or_default(),
            retained
        );
        db.apply_write_batch(&ViewTrimWriteBatch {
            book: Some((root.to_owned(), None, ViewTrimBookState::default())),
            pages: vec![],
        })
        .unwrap();
        assert!(matches!(
            db.load_book_preference_with_fallback(root, None),
            ViewTrimBookLoad::Missing
        ));
        assert_eq!(db.get_page_override("page"), Some(page));
    }

    #[test]
    fn invalid_and_db_errors_are_terminal_and_legacy_migration_is_narrow() {
        let (_temp, db) = open_temp_db();
        let root = Path::new("C:/books/book.zip");
        let child = Path::new("C:/books/book.zip/child");
        db.set_book_state(
            root,
            None,
            ViewTrimBookState {
                preference: ViewTrimBookPreference::Auto,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(matches!(
            db.load_book_preference_with_fallback(child, None),
            ViewTrimBookLoad::Missing
        ));
        assert!(
            db.load_book_preference_with_fallback(child, None)
                .allows_legacy_margin_fit_migration()
        );
        for (json, allowed) in [
            (r#"{"apply_mode":"none"}"#, true),
            (r#"{"apply_mode":"page"}"#, false),
            (r#"{"apply_mode":"auto"}"#, false),
            (r#"{"apply_mode":"book"}"#, false),
            (r#"{"version":2,"preference":"follow_global"}"#, false),
            (r#"{"version":2,"preference":"none"}"#, false),
            (r#"{"version":3,"preference":"auto"}"#, false),
            (r#"{"version":2}"#, false),
        ] {
            db.conn
                .execute(
                    "INSERT OR REPLACE INTO view_trim_books(book_key,state_json) VALUES(?1,?2)",
                    rusqlite::params![book_key(child), json],
                )
                .unwrap();
            let loaded = db.load_book_preference_with_fallback(child, Some(root));
            assert_eq!(
                loaded.allows_legacy_margin_fit_migration(),
                allowed,
                "{json}"
            );
            if json.contains("\"version\":3") || json == r#"{"version":2}"# {
                assert!(matches!(loaded, ViewTrimBookLoad::Invalid(_)));
                assert_eq!(
                    loaded.state_or_default().preference,
                    ViewTrimBookPreference::None
                );
            }
            let after: String = db
                .conn
                .query_row(
                    "SELECT state_json FROM view_trim_books WHERE book_key=?1",
                    [book_key(child)],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(after, json);
        }
        db.conn.execute_batch("DROP TABLE view_trim_books").unwrap();
        let failed = db.load_book_preference_with_fallback(child, Some(root));
        assert!(matches!(failed, ViewTrimBookLoad::Error(_)));
        assert_eq!(
            failed.state_or_default().preference,
            ViewTrimBookPreference::None
        );
        assert!(!failed.allows_legacy_margin_fit_migration());
    }

    #[test]
    fn book_state_roundtrip_and_removal() {
        let (_temp, db) = open_temp_db();
        let path = Path::new(r"C:\Books\Vol1");
        assert!(matches!(
            db.load_book_preference_with_fallback(path, None),
            ViewTrimBookLoad::Missing
        ));

        let state = ViewTrimBookState {
            preference: ViewTrimBookPreference::Manual,
            book_settings: ViewTrimBookSettings {
                enabled: true,
                single: ViewTrimMargins {
                    left: 0.05,
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        db.set_book_state(path, None, state).unwrap();
        assert_eq!(
            db.load_book_preference_with_fallback(path, None)
                .state_or_default(),
            state
        );

        db.set_book_state(path, None, ViewTrimBookState::default())
            .unwrap();
        assert!(matches!(
            db.load_book_preference_with_fallback(path, None),
            ViewTrimBookLoad::Missing
        ));
    }

    #[test]
    fn page_overrides_load_by_escaped_prefix() {
        let (_temp, db) = open_temp_db();
        let keep_key = "c:/imgs/a_[one].png";
        let skip_key = "c:/imgs/a_xone].png";
        let page_override = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            top: 0.04,
            ..Default::default()
        });
        db.set_page_override(keep_key, page_override).unwrap();
        db.set_page_override(
            skip_key,
            ViewTrimPageOverride::from_margins(ViewTrimMargins {
                bottom: 0.07,
                ..Default::default()
            }),
        )
        .unwrap();

        let got = db.load_page_overrides_by_prefix("c:/imgs/a_[");
        assert_eq!(got.len(), 1);
        assert_eq!(got.get(keep_key), Some(&page_override));

        db.remove_page_override(keep_key).unwrap();
        assert!(db.load_page_overrides_by_prefix("c:/imgs/a_[").is_empty());
    }

    #[test]
    fn page_overrides_load_many_returns_only_requested_exact_keys() {
        let (_temp, db) = open_temp_db();
        let page_override = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.03,
            ..Default::default()
        });
        db.set_page_override("c:/a.jpg", page_override).unwrap();
        db.set_page_override("c:/b.jpg", page_override).unwrap();
        let loaded = db.load_page_overrides_many(&["c:/b.jpg", "c:/missing.jpg"]);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("c:/b.jpg"), Some(&page_override));
    }

    #[test]
    fn write_batch_persists_book_and_pages_together() {
        let (_temp, mut db) = open_temp_db();
        let book = PathBuf::from(r"C:\Books\Vol2");
        let book_state = ViewTrimBookState {
            preference: ViewTrimBookPreference::Manual,
            book_settings: ViewTrimBookSettings {
                enabled: true,
                single: ViewTrimMargins {
                    right: 0.08,
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        let page_state = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            bottom: 0.06,
            ..Default::default()
        });
        db.apply_write_batch(&ViewTrimWriteBatch {
            book: Some((book.clone(), None, book_state)),
            pages: vec![("c:/books/vol2/page.jpg".to_string(), page_state)],
        })
        .unwrap();

        assert_eq!(
            db.load_book_preference_with_fallback(&book, None)
                .state_or_default(),
            book_state
        );
        assert_eq!(
            db.load_page_overrides_many(&["c:/books/vol2/page.jpg"])
                .get("c:/books/vol2/page.jpg"),
            Some(&page_state)
        );
    }

    #[test]
    fn read_only_open_never_creates_a_missing_database() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing").join("view_trim.db");

        assert!(
            ViewTrimDb::open_existing_read_only_at(&path)
                .unwrap()
                .is_none()
        );
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
        assert!(
            ViewTrimDb::open_existing_read_only_at(Path::new("invalid\0view_trim.db")).is_err()
        );
    }

    #[test]
    fn read_only_open_reads_existing_rows_but_rejects_writes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("view_trim.db");
        let writable = ViewTrimDb::open_at(&path).unwrap();
        let book = Path::new(r"C:\Books\ReadOnly");
        let state = ViewTrimBookState {
            preference: ViewTrimBookPreference::Manual,
            book_settings: ViewTrimBookSettings {
                enabled: true,
                single: ViewTrimMargins {
                    left: 0.05,
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        writable.set_book_state(book, None, state).unwrap();
        writable
            .set_page_override(
                "c:/books/readonly/page.jpg",
                ViewTrimPageOverride::from_margins(ViewTrimMargins {
                    right: 0.04,
                    ..Default::default()
                }),
            )
            .unwrap();
        drop(writable);

        let read_only = ViewTrimDb::open_existing_read_only_at(&path)
            .unwrap()
            .unwrap();
        assert_eq!(
            read_only
                .load_book_preference_with_fallback(book, None)
                .state_or_default(),
            state
        );
        assert!(
            read_only
                .get_page_override("c:/books/readonly/page.jpg")
                .is_some()
        );
        assert!(
            read_only
                .set_book_state(book, None, ViewTrimBookState::default())
                .is_err()
        );
    }
}
