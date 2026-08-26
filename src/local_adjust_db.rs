//! 補正レイヤーの最小永続管理。
//!
//! `%APPDATA%/mimageviewer/local_adjust.db` に、ページ単位の
//! `Vec<local_adjust_core::LocalAdjustmentLayer>` を JSON として保存する。
//! 中央 DB を authoritative にし、フォルダ移動時の復元用バックアップとして
//! `mimageviewer.dat` にも同じレイヤー配列をミラーする。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use local_adjust_core::LocalAdjustmentLayer;

/// 補正レイヤー DB ハンドル。
pub struct LocalAdjustDb {
    conn: rusqlite::Connection,
}

impl LocalAdjustDb {
    /// DB を開く (なければ作成)。
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 任意のパスで DB を開く。テスト・統合テスト用。
    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS local_adjust_pages (
                page_path   TEXT PRIMARY KEY,
                layers_json TEXT NOT NULL,
                updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        )?;
        Ok(Self { conn })
    }

    /// 起動時に schema 初期化済みの DB を一覧準備 worker から読み取り専用で開く。
    pub fn open_readonly(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_millis(750))?;
        Ok(Self { conn })
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("local_adjust.db")
    }

    /// ページの補正レイヤー配列を取得する。未登録または JSON 破損なら None。
    pub fn get_layers(&self, page_key: &str) -> Option<Vec<LocalAdjustmentLayer>> {
        let json = self.get_layers_json(page_key)?;
        serde_json::from_str(&json).ok()
    }

    pub(crate) fn get_layers_checked(
        &self,
        page_key: &str,
    ) -> Result<Option<Vec<LocalAdjustmentLayer>>, String> {
        use rusqlite::OptionalExtension as _;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT layers_json FROM local_adjust_pages WHERE page_path = ?1")
            .map_err(|error| error.to_string())?;
        let json: Option<String> = stmt
            .query_row([page_key], |row| row.get(0))
            .optional()
            .map_err(|error| error.to_string())?;
        json.map(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))
            .transpose()
    }

    /// worker 側で SQLite 読み込みと JSON 復元を別々に計測するため、
    /// 永続化された JSON 文字列だけを取得する。
    pub fn get_layers_json(&self, page_key: &str) -> Option<String> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT layers_json FROM local_adjust_pages WHERE page_path = ?1")
            .ok()?;
        stmt.query_row([page_key], |row| row.get(0)).ok()
    }

    /// ページの補正レイヤー配列を書き込む。空配列は削除として扱う。
    pub fn set_layers(
        &self,
        page_key: &str,
        layers: &[LocalAdjustmentLayer],
    ) -> Result<(), rusqlite::Error> {
        if layers.is_empty() {
            return self.remove_layers(page_key);
        }
        let json = serde_json::to_string(layers).unwrap_or_else(|_| "[]".to_string());
        self.conn.execute(
            "INSERT INTO local_adjust_pages (page_path, layers_json, updated_at)
             VALUES (?1, ?2, unixepoch())
             ON CONFLICT(page_path) DO UPDATE SET
                layers_json = ?2,
                updated_at = unixepoch()",
            rusqlite::params![page_key, json],
        )?;
        Ok(())
    }

    /// ページの補正レイヤーを削除する。
    pub fn remove_layers(&self, page_key: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM local_adjust_pages WHERE page_path = ?1",
            [page_key],
        )?;
        Ok(())
    }

    pub fn copy_entry_key(&self, from_key: &str, to_key: &str) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO local_adjust_pages (page_path, layers_json, updated_at)
             SELECT ?2, layers_json, unixepoch()
             FROM local_adjust_pages WHERE page_path = ?1
             ON CONFLICT(page_path) DO UPDATE SET
                layers_json = excluded.layers_json,
                updated_at = unixepoch()",
            rusqlite::params![from_key, to_key],
        )?;
        Ok(())
    }

    pub fn move_entry_key(&self, from_key: &str, to_key: &str) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        self.copy_entry_key(from_key, to_key)?;
        self.remove_layers(from_key)
    }

    /// 指定プレフィックスで始まるページキー集合を返す。
    ///
    /// フォルダロード時の hydrate や、サムネイル上のバッジ判定に使う。
    pub fn load_layer_keys(&self, prefix: &str) -> HashSet<String> {
        let mut set = HashSet::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path FROM local_adjust_pages
             WHERE page_path LIKE ?1 ESCAPE '\\'",
        ) else {
            return set;
        };
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('[', "\\[");
        let pattern = format!("{escaped}%");
        let Ok(rows) = stmt.query_map([&pattern], |row| row.get::<_, String>(0)) else {
            return set;
        };
        for row in rows.flatten() {
            set.insert(row);
        }
        set
    }

    /// 補正レイヤー行が存在するページキーを全件返す。
    ///
    /// スマートフィルタの親コンテナ判定用。巨大な `layers_json` は読まない。
    pub fn load_all_layer_keys(&self) -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        let Ok(mut stmt) = self
            .conn
            .prepare_cached("SELECT page_path FROM local_adjust_pages")
        else {
            return set;
        };
        let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
            return set;
        };
        for row in rows.flatten() {
            set.insert(row);
        }
        set
    }

    /// 指定ページキーのうち、補正レイヤー行が存在するキー集合を返す。
    ///
    /// `page_path` は PRIMARY KEY なので、`IN` の exact lookup は既存 index だけで足りる。
    /// グリッド表示のバッジ判定では巨大な `layers_json` を読まず、このキー集合だけを使う。
    pub fn load_existing_layer_keys(&self, page_keys: &[String]) -> HashSet<String> {
        const CHUNK_SIZE: usize = 500;

        let mut set = HashSet::new();
        if page_keys.is_empty() {
            return set;
        }

        let mut unique_keys = Vec::new();
        let mut seen = HashSet::new();
        for key in page_keys {
            if seen.insert(key.as_str()) {
                unique_keys.push(key.as_str());
            }
        }

        for chunk in unique_keys.chunks(CHUNK_SIZE) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT page_path FROM local_adjust_pages WHERE page_path IN ({placeholders})"
            );
            let Ok(mut stmt) = self.conn.prepare_cached(&sql) else {
                continue;
            };
            let Ok(rows) = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter().copied()), |row| {
                    row.get::<_, String>(0)
                })
            else {
                continue;
            };
            for row in rows.flatten() {
                set.insert(row);
            }
        }

        set
    }

    /// 指定プレフィックスで始まるページの補正レイヤー配列を一括取得する。
    pub fn load_layers_by_prefix(
        &self,
        prefix: &str,
    ) -> HashMap<String, Vec<LocalAdjustmentLayer>> {
        let mut map = HashMap::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path, layers_json FROM local_adjust_pages
             WHERE page_path LIKE ?1 ESCAPE '\\'",
        ) else {
            return map;
        };
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('[', "\\[");
        let pattern = format!("{escaped}%");
        let Ok(rows) = stmt.query_map([&pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            return map;
        };
        for (key, json) in rows.flatten() {
            if let Ok(layers) = serde_json::from_str::<Vec<LocalAdjustmentLayer>>(&json) {
                if !layers.is_empty() {
                    map.insert(key, layers);
                }
            }
        }
        map
    }
}

// ── 旧形式マスクの詰め直し ─────────────────────────────────────────────

/// [`repack_legacy_masks`] の結果。ログにそのまま出す。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RepackReport {
    pub scanned: usize,
    pub rewritten: usize,
    /// 読んでから書くまでの間にアプリ側が保存した行。次回起動で拾い直す。
    pub skipped_changed: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// VACUUM 前後のファイルサイズ。書き換えただけでは空きページが残るので、
    /// 利用者に返る容量はこちらで測る。
    pub file_bytes_before: u64,
    pub file_bytes_after: u64,
}

impl RepackReport {
    pub fn did_work(&self) -> bool {
        self.rewritten > 0 || self.skipped_changed > 0
    }
}

/// v1.1.0〜v3.2.0 が書いた「1 画素 1 数値の JSON 配列」マスクを packed 形式へ書き直す。
///
/// 実測で 82 行 954 MB、最大の 1 行が 93 MB あり、そのページを開くたびに JSON の
/// 数値配列を 1 千万個パースしていた。新形式は同じ内容が数十 KB になる。
///
/// **worker スレッドから呼ぶこと。**1 行ずつ読み、旧形式だったものだけを書き戻す。
/// 書き戻しの条件は「読んだ内容から 1 バイトも変わっていない」なので、その間に
/// アプリ側が保存した行を上書きすることはない (飛ばして次回起動で拾う)。
///
/// 移行済み DB では各行が小さいので、毎起動走らせても数 ms で終わる。だから
/// 「済み」フラグを持たない — 古い DB を手で戻された場合も自分で直る。
pub fn repack_legacy_masks(path: &Path) -> RepackReport {
    let mut report = RepackReport::default();
    let Ok(conn) = rusqlite::Connection::open(path) else {
        return report;
    };
    // 本体側の保存と競合したら待つ (既定は 0 で即 SQLITE_BUSY)。
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));

    let keys: Vec<String> = {
        let Ok(mut stmt) = conn.prepare("SELECT page_path FROM local_adjust_pages") else {
            return report;
        };
        let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
            return report;
        };
        rows.flatten().collect()
    };

    for key in keys {
        let Ok(json) = conn.query_row(
            "SELECT layers_json FROM local_adjust_pages WHERE page_path = ?1",
            [&key],
            |row| row.get::<_, String>(0),
        ) else {
            continue;
        };
        report.scanned += 1;

        let before = local_adjust_core::mask_codec::legacy_decode_count();
        let Ok(layers) = serde_json::from_str::<Vec<LocalAdjustmentLayer>>(&json) else {
            continue;
        };
        if local_adjust_core::mask_codec::legacy_decode_count() == before {
            continue; // 既に新形式。
        }
        let Ok(packed) = serde_json::to_string(&layers) else {
            continue;
        };

        // `layers_json = ?3` が楽観ロック。0 行なら誰かが先に書いている。
        match conn.execute(
            "UPDATE local_adjust_pages SET layers_json = ?2
             WHERE page_path = ?1 AND layers_json = ?3",
            rusqlite::params![&key, &packed, &json],
        ) {
            Ok(0) => report.skipped_changed += 1,
            Ok(_) => {
                report.rewritten += 1;
                report.bytes_before += json.len() as u64;
                report.bytes_after += packed.len() as u64;
            }
            Err(error) => {
                crate::logger::log(format!(
                    "local_adjust: mask repack failed for {key}: {error}"
                ));
            }
        }
    }

    // 書き換えただけではファイルは縮まない。実データでは 954 MB ぶんが空きページとして
    // 残る。詰め直した後の中身は数 MB しかないので、VACUUM は実測 0.12 秒で終わる
    // (rusqlite の既定 busy_timeout は 5 秒あるので、本体側の保存はその間待つ)。
    if report.rewritten > 0 {
        let before = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        match conn.execute_batch("VACUUM") {
            Ok(()) => {
                report.file_bytes_before = before;
                report.file_bytes_after =
                    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
            }
            Err(error) => {
                crate::logger::log(format!("local_adjust: VACUUM after repack failed: {error}"));
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_adjust_core::{LocalEffect, LocalMask};

    fn open_temp_db() -> (tempfile::TempDir, LocalAdjustDb) {
        let temp = tempfile::tempdir().unwrap();
        let db = LocalAdjustDb::open_at(&temp.path().join("local_adjust.db")).unwrap();
        (temp, db)
    }

    fn sample_layer(name: &str) -> LocalAdjustmentLayer {
        LocalAdjustmentLayer::new(name, LocalMask::Full, LocalEffect::None)
    }

    #[test]
    fn round_trips_page_layers() {
        let (_temp, db) = open_temp_db();
        let key = "c:/imgs/a.png";
        let layers = vec![sample_layer("base"), sample_layer("finish")];

        db.set_layers(key, &layers).unwrap();

        assert_eq!(db.get_layers(key), Some(layers));
    }

    #[test]
    fn empty_layers_remove_page_entry() {
        let (_temp, db) = open_temp_db();
        let key = "c:/imgs/a.png";
        db.set_layers(key, &[sample_layer("base")]).unwrap();

        db.set_layers(key, &[]).unwrap();

        assert_eq!(db.get_layers(key), None);
    }

    #[test]
    fn load_layer_keys_filters_by_escaped_prefix() {
        let (_temp, db) = open_temp_db();
        db.set_layers("c:/imgs/100%/a.png", &[sample_layer("a")])
            .unwrap();
        db.set_layers("c:/imgs/100x/b.png", &[sample_layer("b")])
            .unwrap();

        let keys = db.load_layer_keys("c:/imgs/100%/");

        assert!(keys.contains("c:/imgs/100%/a.png"));
        assert!(!keys.contains("c:/imgs/100x/b.png"));
    }

    #[test]
    fn load_existing_layer_keys_returns_exact_matches_only() {
        let (_temp, db) = open_temp_db();
        db.set_layers("c:/imgs/a.png", &[sample_layer("a")])
            .unwrap();
        db.set_layers("c:/imgs/sub/b.png", &[sample_layer("b")])
            .unwrap();

        let keys = vec![
            "c:/imgs/a.png".to_string(),
            "c:/imgs/a.png".to_string(),
            "c:/imgs/missing.png".to_string(),
            "c:/imgs/sub".to_string(),
        ];
        let existing = db.load_existing_layer_keys(&keys);

        assert!(existing.contains("c:/imgs/a.png"));
        assert!(!existing.contains("c:/imgs/missing.png"));
        assert!(!existing.contains("c:/imgs/sub"));
        assert_eq!(existing.len(), 1);
    }

    #[test]
    fn load_layers_by_prefix_returns_non_empty_layers() {
        let (_temp, db) = open_temp_db();
        let layers = vec![sample_layer("a")];
        db.set_layers("c:/imgs/a.png", &layers).unwrap();
        db.set_layers("c:/other/b.png", &[sample_layer("b")])
            .unwrap();

        let map = db.load_layers_by_prefix("c:/imgs/");

        assert_eq!(map.get("c:/imgs/a.png"), Some(&layers));
        assert!(!map.contains_key("c:/other/b.png"));
    }
}

#[cfg(test)]
mod repack_tests {
    use super::*;
    use local_adjust_core::{LocalEffect, LocalMask, RasterVectorMask};

    fn legacy_row_json(width: usize, height: usize) -> String {
        // 手で組む。新しい serde を通すと packed になってしまい、移行元にならない。
        let alpha: Vec<String> = (0..width * height)
            .map(|i| {
                if i == 3 {
                    "1.0".into()
                } else {
                    "0.0".to_string()
                }
            })
            .collect();
        format!(
            r#"[{{"name":"layer","enabled":true,"opacity":1.0,
                "mask":{{"RasterVector":{{"width":{width},"height":{height},"alpha":[{}],"shapes":[]}}}},
                "mask_inverted":false,"mask_expand_px":0.0,"mask_feather_px":0.0,
                "effect":"None"}}]"#,
            alpha.join(",")
        )
    }

    #[test]
    fn a_released_number_array_row_is_rewritten_and_keeps_its_content() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("local_adjust.db");
        let db = LocalAdjustDb::open_at(&path).unwrap();
        let legacy = legacy_row_json(40, 40);
        db.conn
            .execute(
                "INSERT INTO local_adjust_pages (page_path, layers_json, updated_at)
                 VALUES ('c:/a.png', ?1, 1)",
                [&legacy],
            )
            .unwrap();
        drop(db);

        let report = repack_legacy_masks(&path);
        assert_eq!(report.rewritten, 1);
        assert_eq!(report.skipped_changed, 0);
        assert!(report.bytes_after * 4 < report.bytes_before);

        let db = LocalAdjustDb::open_at(&path).unwrap();
        let stored = db.get_layers_json("c:/a.png").unwrap();
        assert!(!stored.contains(r#""alpha":["#), "{stored}");
        let layers = db.get_layers("c:/a.png").expect("still readable");
        let LocalMask::RasterVector(mask) = &layers[0].mask else {
            panic!("mask kind changed");
        };
        assert_eq!(mask.alpha.len(), 40 * 40);
        assert_eq!(mask.alpha[3], 1.0);
        assert_eq!(mask.alpha[0], 0.0);
    }

    #[test]
    fn a_second_pass_finds_nothing_left_to_do() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("local_adjust.db");
        let db = LocalAdjustDb::open_at(&path).unwrap();
        db.conn
            .execute(
                "INSERT INTO local_adjust_pages (page_path, layers_json, updated_at)
                 VALUES ('c:/a.png', ?1, 1)",
                [&legacy_row_json(8, 8)],
            )
            .unwrap();
        drop(db);

        assert_eq!(repack_legacy_masks(&path).rewritten, 1);
        let second = repack_legacy_masks(&path);
        assert_eq!(second.scanned, 1);
        assert_eq!(second.rewritten, 0, "the rewrite must be idempotent");
    }

    #[test]
    fn a_row_the_app_saved_meanwhile_is_left_alone() {
        // 楽観ロックの相手。移行が読んだ後にアプリが書いた行を上書きすると、
        // その保存が消える。
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("local_adjust.db");
        let db = LocalAdjustDb::open_at(&path).unwrap();
        db.conn
            .execute(
                "INSERT INTO local_adjust_pages (page_path, layers_json, updated_at)
                 VALUES ('c:/a.png', ?1, 1)",
                [&legacy_row_json(8, 8)],
            )
            .unwrap();

        // 移行が読む内容を控えたうえで、先にアプリ側の保存を入れる。
        let stale = db.get_layers_json("c:/a.png").unwrap();
        let mut mask = RasterVectorMask::empty(8, 8);
        mask.alpha[7] = 1.0;
        let saved = vec![LocalAdjustmentLayer::new(
            "edited",
            LocalMask::RasterVector(mask),
            LocalEffect::None,
        )];
        db.set_layers("c:/a.png", &saved).unwrap();
        drop(db);

        // 控えた古い内容で書き戻そうとする = 移行の中身と同じ条件。
        let conn = rusqlite::Connection::open(&path).unwrap();
        let changed = conn
            .execute(
                "UPDATE local_adjust_pages SET layers_json = 'clobbered'
                 WHERE page_path = 'c:/a.png' AND layers_json = ?1",
                [&stale],
            )
            .unwrap();
        assert_eq!(
            changed, 0,
            "the guard must refuse to overwrite a newer save"
        );

        let db = LocalAdjustDb::open_at(&path).unwrap();
        assert_eq!(db.get_layers("c:/a.png").unwrap()[0].name, "edited");
    }
}

#[cfg(test)]
mod real_data_dry_run {
    /// 実データでの一度きりの確認用。`MIV_REPACK_DRY_RUN=<db path>` を指定したときだけ走る。
    #[test]
    fn repack_a_copy_of_a_real_database() {
        let Ok(path) = std::env::var("MIV_REPACK_DRY_RUN") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let before = std::fs::metadata(&path).unwrap().len();
        let started = std::time::Instant::now();
        let report = super::repack_legacy_masks(&path);
        println!(
            "file {:.1}MB -> {:.1}MB, {:?} in {:.1}s",
            before as f64 / 1e6,
            std::fs::metadata(&path).unwrap().len() as f64 / 1e6,
            report,
            started.elapsed().as_secs_f64()
        );
        assert_eq!(report.skipped_changed, 0);
        // 走査したのに 1 行も書けなかったら、パースで落ちている疑い。
        assert!(report.scanned > 0 && report.rewritten > 0);
    }
}
