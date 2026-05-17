//! 設定 DB の手動復元 / 完全リセット支援。
//!
//! 「現在の設定 (= `settings.db` + WAL 適用後)」と世代バックアップ
//! (`settings.db.bak1` 〜 `settings.db.bak10`) を一覧化し、ユーザーが選んだ世代の
//! 内容で `settings.db` を入れ替える。
//!
//! 2026-05-17: cargo test で **本番 `%APPDATA%\mimageviewer\settings.db` を
//! defaults で踏み潰した事故** に端を発する、ユーザー向け復旧 UI のバックエンド。
//! UI 側 ([`crate::ui_dialogs::settings_restore`]) は復元後に
//! `ViewportCommand::Close` でアプリを終了する前提で組み立てる (= 復元直後に in-memory
//! settings が古いまま `save_full` が走ると、せっかく上書きした `settings.db` を
//! 二次的に踏み潰す事故が起きる)。本モジュールはファイル操作だけで完結させ、
//! `with_db` / `GLOBAL_DB` の状態を直接いじらない。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OpenFlags};

// ──────────────────────────────────────────────────────────────────────
// 公開型
// ──────────────────────────────────────────────────────────────────────

/// バックアップの出どころを識別する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupSource {
    /// 稼働中の `settings.db` (= WAL を適用した有効な状態)
    Current,
    /// 世代バックアップ `settings.db.bak1` 〜 `settings.db.bak10`
    Bak(u8),
}

impl BackupSource {
    /// 表示用ラベル。
    pub fn label(&self) -> String {
        match self {
            BackupSource::Current => "現在の設定".to_string(),
            BackupSource::Bak(n) => format!("バックアップ {n}"),
        }
    }

    /// `<data_dir>` 直下のファイル名。
    pub fn filename(&self) -> String {
        match self {
            BackupSource::Current => "settings.db".to_string(),
            BackupSource::Bak(n) => format!("settings.db.bak{n}"),
        }
    }

    /// 「これは現状の settings.db そのもの」か。
    pub fn is_current(&self) -> bool {
        matches!(self, BackupSource::Current)
    }
}

/// バックアップ 1 件のメタ情報。
#[derive(Debug, Clone)]
pub struct BackupSummary {
    pub source: BackupSource,
    pub mtime: Option<SystemTime>,
    pub size: u64,
    pub favorites: usize,
    pub tags: usize,
    pub video_resume: usize,
    pub vst3_plugins: usize,
    /// 当該 DB を最後に書いた mImageViewer のバージョン。
    /// `schema_meta.app_version` を読むだけなので空も許容する。
    pub app_version: Option<String>,
    /// open / SELECT で出た非致命のエラー (= スキーマが古い / 表が無い等)。
    /// UI 側で「壊れている可能性」として表示する。
    pub partial_error: Option<String>,
}

/// `restore_from` の戻り値: 何を退避してどこに置いたか。
#[derive(Debug)]
pub struct RestoreReport {
    /// `before-restore-<ts>` 退避先パス一覧 (= ユーザーが手動で undo できる)。
    pub snapshot_paths: Vec<PathBuf>,
}

/// `full_reset` の戻り値。
#[derive(Debug)]
pub struct ResetReport {
    pub snapshot_paths: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

/// 失敗時のエラー。
#[derive(Debug)]
pub enum RestoreError {
    Io(std::io::Error),
    /// バックアップ元ファイルが見つからない。
    SourceMissing(PathBuf),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestoreError::Io(e) => write!(f, "I/O エラー: {e}"),
            RestoreError::SourceMissing(p) => {
                write!(f, "復元元が見つかりません: {}", p.display())
            }
        }
    }
}

impl std::error::Error for RestoreError {}

impl From<std::io::Error> for RestoreError {
    fn from(e: std::io::Error) -> Self {
        RestoreError::Io(e)
    }
}

// ──────────────────────────────────────────────────────────────────────
// 一覧
// ──────────────────────────────────────────────────────────────────────

/// `<data_dir>` 直下の現在 + bak1..bak10 を新しい順 (= bak1 番号順) で返す。
/// 存在しない世代はスキップ。各エントリのメタ情報読み取りに失敗しても、
/// `partial_error` を埋めて返す (= 行自体は表示する)。
pub fn list_backups(data_dir: &Path) -> Vec<BackupSummary> {
    let mut out = Vec::new();
    let current = data_dir.join("settings.db");
    if current.exists() {
        out.push(read_summary(BackupSource::Current, &current));
    }
    for n in 1..=10u8 {
        let path = data_dir.join(format!("settings.db.bak{n}"));
        if !path.exists() {
            continue;
        }
        out.push(read_summary(BackupSource::Bak(n), &path));
    }
    out
}

fn read_summary(source: BackupSource, path: &Path) -> BackupSummary {
    let (mtime, size) = match std::fs::metadata(path) {
        Ok(meta) => (meta.modified().ok(), meta.len()),
        Err(_) => (None, 0),
    };
    let mut summary = BackupSummary {
        source,
        mtime,
        size,
        favorites: 0,
        tags: 0,
        video_resume: 0,
        vst3_plugins: 0,
        app_version: None,
        partial_error: None,
    };
    // sqlite を read-only で open。WAL siblings は無視 (bak には -wal が無いし、
    // 現在の settings.db は WAL 適用後の状態を取りたい = 通常 open でよい)。
    let conn_result = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY);
    let conn = match conn_result {
        Ok(c) => c,
        Err(e) => {
            summary.partial_error = Some(format!("DB を開けませんでした: {e}"));
            return summary;
        }
    };
    summary.favorites = count_table(&conn, "favorites").unwrap_or_else(|e| {
        summary
            .partial_error
            .get_or_insert_with(String::new)
            .push_str(&format!("favorites: {e}; "));
        0
    });
    summary.tags = count_table(&conn, "tags").unwrap_or_else(|e| {
        summary
            .partial_error
            .get_or_insert_with(String::new)
            .push_str(&format!("tags: {e}; "));
        0
    });
    summary.video_resume = count_table(&conn, "video_resume_positions").unwrap_or_else(|e| {
        summary
            .partial_error
            .get_or_insert_with(String::new)
            .push_str(&format!("video_resume_positions: {e}; "));
        0
    });
    summary.vst3_plugins = count_table(&conn, "vst3_plugins").unwrap_or_else(|e| {
        summary
            .partial_error
            .get_or_insert_with(String::new)
            .push_str(&format!("vst3_plugins: {e}; "));
        0
    });
    summary.app_version = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'app_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok();
    summary
}

fn count_table(conn: &Connection, table: &str) -> Result<usize, String> {
    // テーブル名は固定リテラル経由でのみ呼ぶ前提 (= SQL インジェクション余地なし)。
    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |r| r.get::<_, i64>(0))
        .map(|n| n as usize)
        .map_err(|e| e.to_string())
}

// ──────────────────────────────────────────────────────────────────────
// 復元 / リセット
// ──────────────────────────────────────────────────────────────────────

/// `source` の内容で `<data_dir>/settings.db` を上書きする。WAL/SHM は削除。
///
/// 副作用:
/// - `crate::settings_db::set_save_suppressed(true)` を立てる (= 復元直後に
///   `with_db` 経由の save が走らないようにする)。
/// - 現状の家族 (`settings.db` / `-wal` / `-shm` / `bak1..bak10`) を
///   `<name>.before-restore-<unix秒>` に **コピー** して退避 (= rename ではない、
///   元ファイルは温存)。
///
/// 復元成功後に caller は **必ずアプリを終了** すること。in-memory の `Settings`
/// は古いままなので、続けて UI 操作 → save() が走ると、せっかく差し替えた
/// `settings.db` を defaults で踏み潰してしまう。
pub fn restore_from(data_dir: &Path, source: &BackupSource) -> Result<RestoreReport, RestoreError> {
    crate::settings_db::set_save_suppressed(true);

    let ts = unix_seconds_now();
    let snapshot_paths = snapshot_current_family(data_dir, ts)?;

    let source_path = data_dir.join(source.filename());
    if !source_path.exists() {
        return Err(RestoreError::SourceMissing(source_path));
    }

    let main_path = data_dir.join("settings.db");
    if source.is_current() {
        // 「現在」=「settings.db そのもの」を選んだ = WAL を捨てるだけ。コピー不要。
    } else {
        // atomic rename ではなく copy。bak ファイル自体は残しておきたい (= 同じ
        // 世代を後で再び復元できるように)。
        copy_replace(&source_path, &main_path)?;
    }

    // WAL/SHM は復元後の状態と整合しなくなるので削除。`remove_file` が
    // NotFound を返したら無視 (= 既に WAL モードを抜けている場合がある)。
    drop_wal_sidecars(data_dir);

    Ok(RestoreReport { snapshot_paths })
}

/// 家族 (`settings.db` + `-wal` + `-shm` + `bak1..bak10`) をすべて削除する。
/// 削除前に同じ `before-restore-<ts>` パターンで退避 (= ユーザーが間違えて
/// 押した場合の救済材料を残す)。
///
/// 削除後、次回起動は `boot_settings_db_inner` で clean install 経路に入る。
pub fn full_reset(data_dir: &Path) -> Result<ResetReport, RestoreError> {
    crate::settings_db::set_save_suppressed(true);

    let ts = unix_seconds_now();
    let snapshot_paths = snapshot_current_family(data_dir, ts)?;

    let mut deleted: Vec<PathBuf> = Vec::new();
    for name in crate::settings_db::family_filenames() {
        let p = data_dir.join(&name);
        match std::fs::remove_file(&p) {
            Ok(()) => deleted.push(p),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(RestoreError::Io(e)),
        }
    }

    Ok(ResetReport {
        snapshot_paths,
        deleted,
    })
}

// ──────────────────────────────────────────────────────────────────────
// 内部ヘルパ
// ──────────────────────────────────────────────────────────────────────

fn snapshot_current_family(data_dir: &Path, ts: u64) -> Result<Vec<PathBuf>, RestoreError> {
    let mut out = Vec::new();
    for name in crate::settings_db::family_filenames() {
        let src = data_dir.join(&name);
        if !src.exists() {
            continue;
        }
        let dst = data_dir.join(format!("{}.before-restore-{}", name, ts));
        // 同一秒内の再復元で衝突するなら上書きする (= 古い退避は失っても致命ではない)。
        std::fs::copy(&src, &dst)?;
        out.push(dst);
    }
    Ok(out)
}

/// `src` を `dst` に **内容コピー** で上書き。Windows で `dst` が他プロセスに
/// 開かれていてもファイル replace が通るよう、`fs::copy` を使う (= rename ではなく
/// truncate + write)。
fn copy_replace(src: &Path, dst: &Path) -> Result<(), RestoreError> {
    std::fs::copy(src, dst)?;
    Ok(())
}

fn drop_wal_sidecars(data_dir: &Path) {
    for name in ["settings.db-wal", "settings.db-shm"] {
        let p = data_dir.join(name);
        // 削除失敗は無視。後続起動で sqlite が WAL を見ても、ベース main は新しい
        // 内容に差し替わっているので checkpoint が来る (= worst case は WAL の
        // 古い page が一時的に混じる可能性、ただし bak はクリーン commit 済みの
        // file なので実害なし)。
        let _ = std::fs::remove_file(&p);
    }
}

fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ──────────────────────────────────────────────────────────────────────
// テスト
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::settings_db::{DataDirOverrideGuard, SettingsDb};

    /// `list_backups` は存在する世代だけ返し、無い世代はスキップする。
    #[test]
    fn list_backups_skips_missing_generations() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        // bak3 だけ用意 (= bak1/bak2 は無い)。
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&Settings::default()).unwrap();
            db.backup_to(&dir.join("settings.db.bak3")).unwrap();
        }
        let summaries = list_backups(dir);
        // 「現在」 + bak3 = 2 件
        assert_eq!(summaries.len(), 2);
        assert!(matches!(summaries[0].source, BackupSource::Current));
        assert!(matches!(summaries[1].source, BackupSource::Bak(3)));
    }

    /// `restore_from(Bak(n))` は `settings.db` を bak の内容で上書きし、
    /// 元の状態を `before-restore-<ts>` に退避する。
    #[test]
    fn restore_from_bak_swaps_main_and_snapshots_prev() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();

        // 「悪い state (= favorites 空)」を settings.db に書き、
        // 「良い state (= favorites 2 件)」を bak1 として保存する。
        let good_state = sample_settings();
        let bad_state = Settings::default();
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&good_state).unwrap();
            db.backup_to(&dir.join("settings.db.bak1")).unwrap();
            db.save_full(&bad_state).unwrap();
        }
        // 復元実行
        let report = restore_from(dir, &BackupSource::Bak(1)).unwrap();
        assert!(!report.snapshot_paths.is_empty(), "snapshot を取っている");
        assert!(
            report.snapshot_paths.iter().any(|p| p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("settings.db.before-restore-")),
            "before-restore-<ts> が作られている"
        );

        // 復元直後に WAL/SHM は消えていること (= SettingsDb::open 前に確認しないと
        // 再 open で新しい WAL が作られてしまう)。
        assert!(!dir.join("settings.db-wal").exists());
        assert!(!dir.join("settings.db-shm").exists());

        // save 抑止が立っている。
        assert!(crate::settings_db::save_suppressed());

        // 復元後の settings.db は good_state を読み返せるはず。
        // 注: with_db は SaveSuppressed が立っているので SettingsDb::open で直接読む。
        let db = SettingsDb::open(dir).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_eq!(loaded.favorites.len(), good_state.favorites.len());
    }

    /// `full_reset` は家族を全削除し、退避を作る。
    #[test]
    fn full_reset_deletes_entire_family_and_snapshots() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&sample_settings()).unwrap();
            db.backup_to(&dir.join("settings.db.bak1")).unwrap();
            db.backup_to(&dir.join("settings.db.bak2")).unwrap();
        }
        let report = full_reset(dir).unwrap();
        // 退避は最低でも main + bak1 + bak2 の 3 つ (WAL/SHM は有無依存)。
        assert!(report.snapshot_paths.len() >= 3);
        // 削除されたものに settings.db / bak1 / bak2 が含まれる。
        let deleted_names: Vec<String> = report
            .deleted
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(deleted_names.iter().any(|n| n == "settings.db"));
        assert!(deleted_names.iter().any(|n| n == "settings.db.bak1"));
        assert!(deleted_names.iter().any(|n| n == "settings.db.bak2"));

        // family 系ファイルは消えている (= 次回起動は clean install 経路)。
        assert!(!dir.join("settings.db").exists());
        assert!(!dir.join("settings.db.bak1").exists());
        assert!(!dir.join("settings.db.bak2").exists());

        // 退避ファイル (before-restore-*) は残っている。
        let entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        let has_snapshot = entries
            .iter()
            .any(|e| e.file_name().to_string_lossy().contains(".before-restore-"));
        assert!(has_snapshot, "before-restore-* が残っているはず");
    }

    fn sample_settings() -> Settings {
        let mut s = Settings::default();
        s.add_favorite("a".into(), std::path::PathBuf::from(r"C:\a"));
        s.add_favorite("b".into(), std::path::PathBuf::from(r"C:\b"));
        s
    }
}
