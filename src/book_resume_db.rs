//! 本 (フォルダ / ZIP / PDF) ごとの「最後に読んだページ」永続管理。
//!
//! `%APPDATA%/mimageviewer/book_resume.db` にコンテナパスごとの最後に表示した
//! ページ index を保存し、アプリ再起動を跨いで読書位置を復元する (動画の
//! `video_resume_positions` の画像本版)。
//!
//! キーは `path_key::normalize` (= ドライブ文字除去・小文字化・スラッシュ統一) で、
//! USB / 外付け HDD のドライブレター変化に追従する点を優先する `spread_db.rs` と
//! 同じ規則。`rotation_db.rs` 等の per-item DB はドライブ文字を保持する別規則なので
//! 混同しないこと (別ドライブの同名パスは同一キーに畳まれるトレードオフがある)。
//!
//! 値は `items` 内の index。ZIP/PDF は列挙順が決定的なので index が安定する。
//! 通常フォルダはファイル追加削除で多少ずれるが、その場合は復元時に範囲・種別を
//! 検証して妥当でなければ先頭にフォールバックする (呼び出し側の責務)。
//!
//! ページ送りのたびに書き込みが走るため、**書き込みは [`BookResumeWriter`] の専用
//! スレッドへ逃がして UI スレッドの同期 SQLite I/O を避ける**。読み出し
//! ([`BookResumeDb::get`]) は本を開くときに 1 回だけなので UI スレッド同期のまま。

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use crate::path_key;

/// 読書位置 DB ハンドル
pub struct BookResumeDb {
    conn: rusqlite::Connection,
}

impl BookResumeDb {
    /// DB を開く (なければ作成)
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        // 読み (UI スレッド) と書き ([`BookResumeWriter`] スレッド) で 2 接続が同じ
        // ファイルを触るため、稀な競合で SQLITE_BUSY を即時エラーにせず待たせる。
        conn.busy_timeout(Duration::from_secs(3))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS book_resume (
                path TEXT PRIMARY KEY,
                page INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        Ok(Self { conn })
    }

    /// DB ファイルのパス
    fn db_path() -> PathBuf {
        crate::data_dir::get().join("book_resume.db")
    }

    /// コンテナの最後に読んだページ index を取得。未登録なら None。
    pub fn get(&self, path: &Path) -> Option<usize> {
        let key = normalize_path(path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT page FROM book_resume WHERE path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| {
            let v: i64 = row.get(0)?;
            Ok(v.max(0) as usize)
        })
        .ok()
    }

    /// 最後に読んだページ index を保存する。
    pub fn set(&self, path: &Path, page: usize) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        self.conn.execute(
            "INSERT INTO book_resume (path, page) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET page = ?2",
            rusqlite::params![key, page as i64],
        )?;
        Ok(())
    }

    /// 1 件削除 (リセット)
    pub fn remove(&self, path: &Path) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        self.conn
            .execute("DELETE FROM book_resume WHERE path = ?1", [&key])?;
        Ok(())
    }

    /// 全レコードを削除 (リセット)
    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM book_resume", [])
    }

    /// 登録件数
    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM book_resume", [], |row| row.get(0))
            .unwrap_or(0)
    }
}

fn normalize_path(path: &Path) -> String {
    path_key::normalize(path)
}

/// 読書位置の書き込みを UI スレッドから外す background writer。自前の write 用
/// Connection を持つ専用スレッドへ `(path, page)` を送って upsert する。ページ送りの
/// たびに走る書き込みで UI が引っかからないようにするのが目的 ([docs/ui-responsiveness.md]
/// の「UI スレッドの同期 I/O は worker 化」)。読み出しは本 open 時の 1 回だけなので
/// `BookResumeDb` 側 (UI スレッド) のままにしている。
pub struct BookResumeWriter {
    /// `Option` なのは `Drop` で先に Sender を落として writer スレッドの recv ループを
    /// 終了させてから join するため。
    tx: Option<mpsc::Sender<(PathBuf, usize)>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl BookResumeWriter {
    /// writer スレッドを spawn する。DB を開けなければ `None` (= 書き込みは no-op)。
    pub fn spawn() -> Option<Self> {
        let (tx, rx) = mpsc::channel::<(PathBuf, usize)>();
        let spawned = std::thread::Builder::new()
            .name("book-resume-writer".into())
            .spawn(move || {
                let db = match BookResumeDb::open() {
                    Ok(db) => db,
                    Err(e) => {
                        crate::logger::log(format!("book-resume writer: DB open failed: {e}"));
                        return;
                    }
                };
                // tx が全て drop されるまで (= App 終了まで) 受信し続ける。channel に
                // 溜まっている分は Disconnected 前に drain されるので取りこぼしは無い。
                while let Ok((path, page)) = rx.recv() {
                    if let Err(e) = db.set(&path, page) {
                        crate::logger::log(format!("book-resume writer: set failed: {e}"));
                    }
                }
            });
        match spawned {
            Ok(handle) => Some(Self {
                tx: Some(tx),
                handle: Some(handle),
            }),
            Err(e) => {
                crate::logger::log(format!("book-resume writer: thread spawn failed: {e}"));
                None
            }
        }
    }

    /// 読書位置を非同期で記録する (送るだけ・UI スレッドはブロックしない)。
    pub fn record(&self, path: &Path, page: usize) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((path.to_path_buf(), page));
        }
    }
}

impl Drop for BookResumeWriter {
    fn drop(&mut self) {
        // Sender を先に落とすと writer の `rx.recv()` がキュー分を drain し切ってから
        // `Err` を返してループを抜ける。その後 join して、終了時に積んでいた書き込みが
        // ディスクへ反映されるのを待つ (デタッチのままだとプロセス終了で取りこぼす)。
        self.tx = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_resume_set_get_remove() {
        let db = BookResumeDb::open().unwrap();
        let p = Path::new("C:/manga/book.zip");

        // テスト分離: 既存レコードを消してから
        db.remove(p).unwrap();
        assert!(db.get(p).is_none());

        db.set(p, 42).unwrap();
        assert_eq!(db.get(p), Some(42));

        // 上書き
        db.set(p, 7).unwrap();
        assert_eq!(db.get(p), Some(7));

        // 削除
        db.remove(p).unwrap();
        assert!(db.get(p).is_none());
    }
}
