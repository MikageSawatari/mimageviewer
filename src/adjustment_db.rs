//! 画像補正のページ個別設定を永続管理する。
//!
//! `%APPDATA%/mimageviewer/adjustment.db` に `page_params` テーブルとして保存する。
//! 旧 (v0.6.0 開発版) の `presets` テーブル / preset_idx 方式は廃止。
//! 表示時の有効パラメータは `page_params.get(page) ?? settings.global_preset`。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::adjustment::AdjustParams;

/// 補正設定 DB ハンドル。
pub struct AdjustmentDb {
    conn: rusqlite::Connection,
}

impl AdjustmentDb {
    /// DB を開く (なければ作成)。旧スキーマが残っていれば破棄して作り直す。
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 任意のパスで DB を開く。テスト・統合テスト用。
    /// 通常のランタイムパス (`%APPDATA%/mimageviewer/adjustment.db`) を使いたい場合は
    /// 引数なしの [`open`] を使うこと。
    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        // 未リリース機能なのでマイグレーションは行わず旧テーブルを破棄する。
        //
        // `sidecar_sync`: サイドカー (mimageviewer.dat) を最後に import した時の
        // ファイル mtime を folder_key ごとに記録する。フォルダ切替の度に
        // `fs::metadata(sidecar)` で取った mtime と突き合わせ、一致するなら
        // `read_to_string` + parse + import をまるごとスキップするための fast-path。
        // フォルダ移動・外部ツールでサイドカーを更新したケースだけ slow-path に落ちる。
        // 未リリース機能なので旧 `presets` / `page_presets` は無条件に捨てる。
        conn.execute_batch(
            "DROP TABLE IF EXISTS presets;
             DROP TABLE IF EXISTS page_presets;
             CREATE TABLE IF NOT EXISTS page_params (
                page_path TEXT PRIMARY KEY,
                params_json TEXT NOT NULL
             );",
        )?;

        // sidecar_sync は「旧スキーマ (`synced_at INTEGER NOT NULL` あり) のとき
        // **だけ** 捨てて作り直す」。毎回 DROP すると再起動ごとに mtime キャッシュが
        // 吹っ飛び、各フォルダ初回訪問でサイドカー import が再実行されてしまう
        // (Codex P3)。旧 schema 判定は PRAGMA table_info で列の有無を見る。
        let has_legacy_synced_at = {
            let mut stmt = conn.prepare("PRAGMA table_info(sidecar_sync)")?;
            let cols: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .filter_map(Result::ok)
                .collect();
            cols.iter().any(|c| c == "synced_at")
        };
        if has_legacy_synced_at {
            conn.execute_batch("DROP TABLE IF EXISTS sidecar_sync;")?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sidecar_sync (
                folder_key    TEXT PRIMARY KEY,
                sidecar_mtime INTEGER NOT NULL
             );",
        )?;

        // お気に入り単位の標準パラメータ。
        // 削除済みお気に入りの行は残っても解決で `find_nearest_favorite` が None を返すため
        // 無害。起動時に `prune_favorite_params` で掃除する。
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS favorite_params (
                favorite_id TEXT PRIMARY KEY,
                params_json TEXT NOT NULL
             );",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("adjustment.db")
    }

    /// ページのパラメータを取得する。未登録なら None。
    pub fn get_page_params(&self, page_key: &str) -> Option<AdjustParams> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT params_json FROM page_params WHERE page_path = ?1")
            .ok()?;
        let json: String = stmt.query_row([page_key], |row| row.get(0)).ok()?;
        serde_json::from_str(&json).ok()
    }

    /// ページのパラメータを書き込む。
    ///
    /// 削除判定 (個別保存する意味があるか) は呼び出し側の責務。
    /// 旧バージョンでは `params.is_removable()` で内部的に削除に振り分けていたが、
    /// グローバルが非デフォルトのとき「個別で AI を OFF にする」上書きを保存したい
    /// ケースを取り逃すため、is_removable 判定を呼び出し側 (`App::set_page_params`) に
    /// 移した。明示的に削除したいときは `remove_page_params` を呼ぶこと。
    pub fn set_page_params(
        &self,
        page_key: &str,
        params: &AdjustParams,
    ) -> Result<(), rusqlite::Error> {
        let json = serde_json::to_string(params).unwrap_or_default();
        self.conn.execute(
            "INSERT INTO page_params (page_path, params_json) VALUES (?1, ?2)
             ON CONFLICT(page_path) DO UPDATE SET params_json = ?2",
            rusqlite::params![page_key, json],
        )?;
        Ok(())
    }

    /// ページのパラメータ個別設定を削除する。
    pub fn remove_page_params(&self, page_key: &str) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM page_params WHERE page_path = ?1", [page_key])?;
        Ok(())
    }

    pub fn copy_page_params_key(
        &self,
        from_key: &str,
        to_key: &str,
    ) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO page_params (page_path, params_json)
             SELECT ?2, params_json FROM page_params WHERE page_path = ?1
             ON CONFLICT(page_path) DO UPDATE SET params_json = excluded.params_json",
            rusqlite::params![from_key, to_key],
        )?;
        Ok(())
    }

    pub fn move_page_params_key(
        &self,
        from_key: &str,
        to_key: &str,
    ) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        self.copy_page_params_key(from_key, to_key)?;
        self.remove_page_params(from_key)
    }

    /// 複数ページに同じパラメータを一括書込する (「全画像に適用」ボタン用)。
    ///
    /// 削除判定 (= グローバルと等価かどうか) は呼び出し側の責務。
    /// グローバルと等価な params を渡したい場合は `remove_page_params_bulk` を使う。
    pub fn set_page_params_bulk(
        &mut self,
        page_keys: &[String],
        params: &AdjustParams,
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        let json = serde_json::to_string(params).unwrap_or_default();
        let mut stmt = tx.prepare(
            "INSERT INTO page_params (page_path, params_json) VALUES (?1, ?2)
             ON CONFLICT(page_path) DO UPDATE SET params_json = ?2",
        )?;
        for key in page_keys {
            stmt.execute(rusqlite::params![key, json])?;
        }
        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    /// 複数ページの個別パラメータを一括削除する (「全画像から削除」ボタン用)。
    pub fn remove_page_params_bulk(&mut self, page_keys: &[String]) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        let mut stmt = tx.prepare("DELETE FROM page_params WHERE page_path = ?1")?;
        for key in page_keys {
            stmt.execute([key])?;
        }
        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    // ── サイドカー同期状態 (fast-path 用) ─────────────────────────────

    /// このフォルダについて最後に import したサイドカーの mtime (UNIX 秒) を返す。
    /// 未登録なら None。
    pub fn sidecar_sync_get(&self, folder_key: &str) -> Option<i64> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT sidecar_mtime FROM sidecar_sync WHERE folder_key = ?1")
            .ok()?;
        stmt.query_row([folder_key], |row| row.get::<_, i64>(0))
            .ok()
    }

    /// サイドカー同期状態を upsert。import 成功時に最新 mtime を残す。
    pub fn sidecar_sync_upsert(
        &self,
        folder_key: &str,
        sidecar_mtime: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO sidecar_sync (folder_key, sidecar_mtime) VALUES (?1, ?2)
             ON CONFLICT(folder_key) DO UPDATE SET sidecar_mtime = ?2",
            rusqlite::params![folder_key, sidecar_mtime],
        )?;
        Ok(())
    }

    /// サイドカー同期状態を削除 (サイドカーが削除されたフォルダに追従する用)。
    pub fn sidecar_sync_clear(&self, folder_key: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM sidecar_sync WHERE folder_key = ?1",
            [folder_key],
        )?;
        Ok(())
    }

    // ── お気に入り単位の標準パラメータ ─────────────────────────────

    /// 全お気に入りの標準パラメータを読み込む (起動時に 1 回)。
    pub fn load_all_favorite_params(&self) -> HashMap<Uuid, AdjustParams> {
        let mut map = HashMap::new();
        let Ok(mut stmt) = self
            .conn
            .prepare("SELECT favorite_id, params_json FROM favorite_params")
        else {
            return map;
        };
        let Ok(rows) = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let json: String = row.get(1)?;
            Ok((id, json))
        }) else {
            return map;
        };
        for row in rows.flatten() {
            if let (Ok(id), Ok(params)) = (
                Uuid::parse_str(&row.0),
                serde_json::from_str::<AdjustParams>(&row.1),
            ) {
                map.insert(id, params);
            }
        }
        map
    }

    /// お気に入りの標準パラメータを書き込む。
    pub fn set_favorite_params(
        &self,
        favorite_id: Uuid,
        params: &AdjustParams,
    ) -> Result<(), rusqlite::Error> {
        let json = serde_json::to_string(params).unwrap_or_default();
        self.conn.execute(
            "INSERT INTO favorite_params (favorite_id, params_json) VALUES (?1, ?2)
             ON CONFLICT(favorite_id) DO UPDATE SET params_json = ?2",
            rusqlite::params![favorite_id.to_string(), json],
        )?;
        Ok(())
    }

    /// お気に入りの標準パラメータを削除する。
    pub fn remove_favorite_params(&self, favorite_id: Uuid) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM favorite_params WHERE favorite_id = ?1",
            [favorite_id.to_string()],
        )?;
        Ok(())
    }

    /// `keep` に含まれない favorite_id の行を削除する (起動時 orphan cleanup)。
    /// お気に入りが削除された後もロジック上は無害だが、DB が肥大化しないよう定期的に掃除する。
    pub fn prune_favorite_params(&self, keep: &HashSet<Uuid>) -> Result<usize, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT favorite_id FROM favorite_params")?;
        let rows: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);
        let mut removed = 0usize;
        for id_str in rows {
            let remove = match Uuid::parse_str(&id_str) {
                Ok(id) => !keep.contains(&id),
                Err(_) => true, // 破損した ID は掃除する
            };
            if remove {
                self.conn.execute(
                    "DELETE FROM favorite_params WHERE favorite_id = ?1",
                    [&id_str],
                )?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// コンテナ配下の全ページ個別パラメータを一括読込する。
    /// `prefix` はコンテナパスの正規化文字列。
    pub fn load_page_params(&self, prefix: &str) -> HashMap<String, AdjustParams> {
        let mut map = HashMap::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path, params_json FROM page_params WHERE page_path LIKE ?1 ESCAPE '\\'",
        ) else {
            return map;
        };
        let pattern = format!("{}%", escape_like_pattern(prefix));
        let Ok(rows) = stmt.query_map([&pattern], |row| {
            let path: String = row.get(0)?;
            let json: String = row.get(1)?;
            Ok((path, json))
        }) else {
            return map;
        };
        for row in rows.flatten() {
            if let Ok(params) = serde_json::from_str::<AdjustParams>(&row.1) {
                map.insert(row.0, params);
            }
        }
        map
    }
}

/// パスを正規化 (小文字化 + バックスラッシュ→スラッシュ)。
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
}

/// ZIP/PDF コンテナ内のページ単位キーを構築する。
///
/// `<normalize(container_path)>::<lower(entry_or_page_label)>` という形は
/// `App::page_path_key` (ZipImage / PdfPage 分岐) と `global_search_ui::hit_rating_key`
/// の両方で同じ rating_db / adjustment_db キーを生み出す必要があるため、
/// フォーマットドリフト (= 書き込み側と読み出し側でキーがずれて rating が消える)
/// を防ぐ目的で 1 箇所に集約している。
pub fn zip_entry_key(container_path: &Path, entry: &str) -> String {
    format!(
        "{}::{}",
        normalize_path(container_path),
        entry.to_lowercase()
    )
}

/// SQL `LIKE` の特殊文字 (`\`, `%`, `_`, `[`) を `\` でエスケープする。
/// 呼び出し側は `ESCAPE '\'` 指定の prepared statement で使う。
/// 末尾の `%` は呼び出し側で付与する想定 (prefix 一致なのか完全一致なのかは用途依存)。
pub fn escape_like_pattern(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
        .replace('[', "\\[")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_page_params_roundtrip() {
        // 2026-05-17: 旧版は `AdjustmentDb::open()` を使っていたが、これは内部で
        // `data_dir::get()` 経由で APPDATA に DB を作るため、`cfg(test)` の
        // `data_dir::default()` panic ガードに当たる。`open_at` で temp path に
        // 明示開きに変更 (= 同 module の `db_favorite_params_roundtrip_and_prune` と同方式)。
        let tmp = std::env::temp_dir().join(format!(
            "mimageviewer_page_params_test_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        let db = AdjustmentDb::open_at(&tmp).unwrap();
        let page = "c:/test/folder/page001.jpg";
        // クリーンな状態を保証
        db.remove_page_params(page).unwrap();
        assert!(db.get_page_params(page).is_none());

        let mut params = AdjustParams::default();
        params.brightness = 30.0;
        db.set_page_params(page, &params).unwrap();
        let loaded = db.get_page_params(page).unwrap();
        assert_eq!(loaded.brightness, 30.0);

        // identity でも DB は保存する (削除判定は呼び出し側 App::set_page_params に移った)
        // 個別を「グローバルが AI ON のとき AI OFF として上書き」したいケースを保存できるように。
        db.set_page_params(page, &AdjustParams::default()).unwrap();
        let loaded_identity = db.get_page_params(page).unwrap();
        assert!(loaded_identity.is_identity());

        // 明示削除すれば消える
        db.remove_page_params(page).unwrap();
        assert!(db.get_page_params(page).is_none());

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn db_favorite_params_roundtrip_and_prune() {
        let tmp = std::env::temp_dir().join(format!(
            "mimageviewer_fav_params_test_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        let db = AdjustmentDb::open_at(&tmp).unwrap();

        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut pa = AdjustParams::default();
        pa.brightness = 10.0;
        let mut pb = AdjustParams::default();
        pb.saturation = 20.0;

        db.set_favorite_params(a, &pa).unwrap();
        db.set_favorite_params(b, &pb).unwrap();
        let loaded = db.load_all_favorite_params();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(&a).unwrap().brightness, 10.0);
        assert_eq!(loaded.get(&b).unwrap().saturation, 20.0);

        // b だけ残す → a は prune 対象
        let mut keep = HashSet::new();
        keep.insert(b);
        let removed = db.prune_favorite_params(&keep).unwrap();
        assert_eq!(removed, 1);
        let loaded = db.load_all_favorite_params();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key(&b));
        assert!(!loaded.contains_key(&a));

        // 明示削除
        db.remove_favorite_params(b).unwrap();
        assert!(db.load_all_favorite_params().is_empty());

        let _ = std::fs::remove_file(&tmp);
    }
}
