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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

use crate::path_key;

/// HUDと同じ読み順で記録したanchorの1-based位置。内容変更後も次の記録まで保持する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadingMeterValue {
    pub ordinal: usize,
    pub total: usize,
}

impl ReadingMeterValue {
    pub fn new(ordinal: usize, total: usize) -> Option<Self> {
        (ordinal > 0 && ordinal <= total && i64::try_from(total).is_ok())
            .then_some(Self { ordinal, total })
    }

    pub fn fraction(self) -> f32 {
        self.ordinal as f32 / self.total as f32
    }
}

pub(crate) struct MeterSnapshot {
    pub values: HashMap<String, Option<ReadingMeterValue>>,
}

pub(crate) type MeterReadResult = Result<MeterSnapshot, String>;

/// 読書位置 DB ハンドル
pub struct BookResumeDb {
    conn: rusqlite::Connection,
}

impl BookResumeDb {
    /// DB を開く (なければ作成)
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 指定 DB を通常と同じスキーマ初期化経路で開く。
    pub(crate) fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
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

    /// writer起動時だけ実行する。旧2列・既存行を保持し、失敗時は全ALTERをrollback。
    fn migrate_meter_columns(&mut self) -> Result<(), rusqlite::Error> {
        let columns = self
            .conn
            .prepare("PRAGMA table_info(book_resume)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        let tx = self.conn.transaction()?;
        for column in ["page_ordinal", "page_total"] {
            if !columns.iter().any(|existing| existing == column) {
                tx.execute_batch(&format!(
                    "ALTER TABLE book_resume ADD COLUMN {column} INTEGER"
                ))?;
            }
        }
        tx.commit()
    }

    fn set_meter(
        &self,
        path: &Path,
        page: usize,
        value: Option<ReadingMeterValue>,
    ) -> Result<(), rusqlite::Error> {
        let value = value.and_then(|v| ReadingMeterValue::new(v.ordinal, v.total));
        self.conn.execute(
            "INSERT INTO book_resume (path,page,page_ordinal,page_total)
             VALUES (?1,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET
             page=excluded.page,page_ordinal=excluded.page_ordinal,
             page_total=excluded.page_total",
            rusqlite::params![
                normalize_path(path),
                page as i64,
                value.map(|v| v.ordinal as i64),
                value.map(|v| v.total as i64)
            ],
        )?;
        Ok(())
    }

    fn read_meters(&self) -> Result<MeterSnapshot, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT path,page_ordinal,page_total FROM book_resume")?;
        let mut rows = stmt.query([])?;
        let mut snapshot = MeterSnapshot {
            values: HashMap::new(),
        };
        while let Some(row) = rows.next()? {
            let key = row.get(0)?;
            let mut value = None;
            let tuple = (row.get::<_, Option<i64>>(1), row.get::<_, Option<i64>>(2));
            if let (Ok(Some(ordinal)), Ok(Some(total))) = tuple
                && let (Ok(ordinal), Ok(total)) = (usize::try_from(ordinal), usize::try_from(total))
                && let Some(valid) = ReadingMeterValue::new(ordinal, total)
            {
                value = Some(valid);
            }
            snapshot.values.insert(key, value);
        }
        Ok(snapshot)
    }
}

fn normalize_path(path: &Path) -> String {
    path_key::normalize(path)
}

/// 読書位置の書き込みを UI スレッドから外す background writer。自前の write 用
/// Connection を持つ専用スレッドへ `(path, page, meter)` を送って upsert する。ページ送りの
/// たびに走る書き込みで UI が引っかからないようにするのが目的 ([docs/ui-responsiveness.md]
/// の「UI スレッドの同期 I/O は worker 化」)。メーターの移行/全行読込/clearも同じFIFO。
/// 従来のraw位置復元の読み出しは `BookResumeDb` 側に維持する。
enum Command {
    Record(PathBuf, usize, Option<ReadingMeterValue>),
    Read(mpsc::Sender<MeterReadResult>),
    Clear(mpsc::Sender<Result<usize, String>>),
    #[cfg(test)]
    Pause(mpsc::Sender<()>, mpsc::Receiver<()>),
}

pub struct BookResumeWriter {
    /// `Option` なのは `Drop` で先に Sender を落として writer スレッドの recv ループを
    /// 終了させてから join するため。
    tx: Option<mpsc::Sender<Command>>,
    handle: Option<std::thread::JoinHandle<()>>,
    pending: Arc<AtomicUsize>,
}

impl BookResumeWriter {
    /// writer スレッドを spawn する。DB を開けなければ `None` (= 書き込みは no-op)。
    pub fn spawn() -> Option<Self> {
        Self::spawn_at(BookResumeDb::db_path())
    }

    pub(crate) fn spawn_at(path: PathBuf) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Command>();
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_pending = Arc::clone(&pending);
        let spawned = std::thread::Builder::new()
            .name("book-resume-writer".into())
            .spawn(move || {
                let mut db = match BookResumeDb::open_at(&path) {
                    Ok(db) => db,
                    Err(e) => {
                        crate::logger::log(format!("book-resume writer: DB open failed: {e}"));
                        // Receiverを閉じると後続要求もDisconnectedとして完了する。
                        return;
                    }
                };
                let meter_ready = match db.migrate_meter_columns() {
                    Ok(()) => true,
                    Err(e) => {
                        crate::logger::log(format!(
                            "book-resume writer: meter migration failed: {e}"
                        ));
                        false
                    }
                };
                // tx が全て drop されるまで (= App 終了まで) 受信し続ける。channel に
                // 溜まっている分は Disconnected 前に drain されるので取りこぼしは無い。
                while let Ok(command) = rx.recv() {
                    match command {
                        Command::Record(path, page, value) => {
                            let result = if meter_ready {
                                db.set_meter(&path, page, value)
                            } else {
                                db.set(&path, page)
                            };
                            if let Err(e) = result {
                                crate::logger::log(format!("book-resume writer: set failed: {e}"));
                            }
                        }
                        Command::Read(tx) => {
                            let result = if meter_ready {
                                db.read_meters().map_err(|e| e.to_string())
                            } else {
                                Err("meter schema unavailable".into())
                            };
                            let _ = tx.send(result);
                        }
                        Command::Clear(tx) => {
                            let _ = tx.send(db.clear_all().map_err(|e| e.to_string()));
                        }
                        #[cfg(test)]
                        Command::Pause(entered, resume) => {
                            let _ = entered.send(());
                            let _ = resume.recv();
                        }
                    }
                    worker_pending.fetch_sub(1, Ordering::Release);
                }
            });
        match spawned {
            Ok(handle) => Some(Self {
                tx: Some(tx),
                handle: Some(handle),
                pending,
            }),
            Err(e) => {
                crate::logger::log(format!("book-resume writer: thread spawn failed: {e}"));
                None
            }
        }
    }

    /// 読書位置を非同期で記録する (送るだけ・UI スレッドはブロックしない)。
    fn send(&self, command: Command) -> bool {
        self.pending.fetch_add(1, Ordering::AcqRel);
        if self.tx.as_ref().is_some_and(|tx| tx.send(command).is_ok()) {
            true
        } else {
            self.pending.fetch_sub(1, Ordering::Release);
            false
        }
    }

    pub fn record(&self, path: &Path, page: usize, value: Option<ReadingMeterValue>) -> bool {
        self.send(Command::Record(path.to_path_buf(), page, value))
    }

    pub(crate) fn read_all(&self) -> mpsc::Receiver<MeterReadResult> {
        let (tx, rx) = mpsc::channel();
        self.send(Command::Read(tx));
        rx
    }

    pub(crate) fn clear(&self) -> mpsc::Receiver<Result<usize, String>> {
        let (tx, rx) = mpsc::channel();
        self.send(Command::Clear(tx));
        rx
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
            && self.pending.load(Ordering::Acquire) > 0
    }

    #[cfg(test)]
    pub(crate) fn pause_for_test(&self) -> mpsc::Sender<()> {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        assert!(self.send(Command::Pause(entered_tx, resume_rx)));
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        resume_tx
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
    fn book_resume_meter_migration_preserves_legacy_and_nulls() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("resume.db");
        let mut db = BookResumeDb::open_at(&path).unwrap();
        let book = Path::new("C:/books/a.zip");
        db.set(book, 7).unwrap();
        db.migrate_meter_columns().unwrap();
        db.migrate_meter_columns().unwrap();
        let columns = db
            .conn
            .prepare("PRAGMA table_info(book_resume)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(columns, ["path", "page", "page_ordinal", "page_total"]);
        assert_eq!(db.get(book), Some(7));
        assert_eq!(
            db.read_meters().unwrap().values[&normalize_path(book)],
            None
        );
        let meter = ReadingMeterValue::new(2, 4).unwrap();
        db.set_meter(book, 8, Some(meter)).unwrap();
        // 出荷済み2列版のupsert/SELECTをそのまま実行できる。新列が残るのは合意済み。
        db.set(book, 9).unwrap();
        assert_eq!(db.get(book), Some(9));
        assert_eq!(
            db.read_meters().unwrap().values[&normalize_path(book)],
            Some(meter)
        );
        db.set_meter(book, 10, None).unwrap();
        assert_eq!(
            db.read_meters().unwrap().values[&normalize_path(book)],
            None
        );
    }

    #[test]
    fn book_resume_meter_invalid_columns_are_hidden() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = BookResumeDb::open_at(&tmp.path().join("resume.db")).unwrap();
        db.migrate_meter_columns().unwrap();
        for (idx, (ordinal, total)) in [(0, 5), (1, 0), (6, 5), (-1, 5)].into_iter().enumerate() {
            db.conn
                .execute(
                    "INSERT INTO book_resume VALUES (?1,0,?2,?3)",
                    rusqlite::params![format!("bad{idx}"), ordinal, total],
                )
                .unwrap();
        }
        let snapshot = db.read_meters().unwrap();
        assert_eq!(snapshot.values.len(), 4);
        assert!(snapshot.values.values().all(Option::is_none));
        assert!(ReadingMeterValue::new(0, 1).is_none());
        assert!(ReadingMeterValue::new(1, 0).is_none());
        assert!(ReadingMeterValue::new(2, 1).is_none());
        assert_eq!(ReadingMeterValue::new(1, 1).unwrap().fraction(), 1.0);
    }

    #[test]
    fn book_resume_meter_failed_alter_rolls_back_and_raw_record_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = BookResumeDb::open_at(&tmp.path().join("resume.db")).unwrap();
        // SQLite既定列上限直前: 1列追加成功後に2列目が失敗する決定的なrollback検証。
        let columns = (0..1997)
            .map(|idx| format!(",legacy_{idx} INTEGER"))
            .collect::<String>();
        db.conn.execute_batch(&format!("DROP TABLE book_resume; CREATE TABLE book_resume(path TEXT PRIMARY KEY,page INTEGER NOT NULL DEFAULT 0{columns})")).unwrap();
        assert!(db.migrate_meter_columns().is_err());
        let columns: usize = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('book_resume') WHERE name='page_ordinal'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 0);
        let book = Path::new("C:/books/a.zip");
        db.set(book, 12).unwrap();
        assert_eq!(db.get(book), Some(12));
    }

    #[test]
    fn book_resume_meter_worker_record_read_clear_fifo() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = BookResumeWriter::spawn_at(tmp.path().join("resume.db")).unwrap();
        let release = writer.pause_for_test();
        let book = Path::new("C:/books/a.zip");
        let value = ReadingMeterValue::new(3, 10).unwrap();
        assert!(writer.record(book, 9, Some(value)));
        assert!(writer.is_busy());
        let read = writer.read_all();
        assert!(matches!(read.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert_eq!(
            read.recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .values[&normalize_path(book)],
            Some(value)
        );
        assert_eq!(
            writer
                .clear()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap(),
            1
        );
        assert!(
            writer
                .read_all()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .values
                .is_empty()
        );
        drop(writer);
    }

    #[test]
    fn book_resume_set_get_remove() {
        // data_dir の test override を自分で張る。張らないと、たまたま生きている他
        // テストのガードへ相乗りして同じ DB を共有する。同型の相乗りは rotation_db で
        // 実際に CannotOpen を起こした (2026-09-01、リリースの test gate)。こちらは
        // open() が親ディレクトリを作り直すぶん落ちにくいだけで、分離できていない点は同じ。
        let _guard = crate::data_dir::TestDataDirGuard::new();
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
