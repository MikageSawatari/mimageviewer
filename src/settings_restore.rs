//! 設定 DB の手動復元 / 完全リセット支援。
//!
//! 「現在の設定 (= `settings.db` + WAL 適用後)」と世代バックアップ
//! (`settings.db.bak1` 〜 `settings.db.bak10`)、アップグレード前スナップショットを
//! 一覧化し、ユーザーが選んだ世代の内容で `settings.db` を入れ替える。
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
    /// バージョン跨ぎ前の `settings.db.preupgrade-v<version>`。
    /// String は data_dir の列挙で得た安全なファイル名 suffix のみを保持する。
    PreUpgrade(String),
}

impl BackupSource {
    /// 表示用ラベル。
    pub fn label(&self) -> String {
        match self {
            BackupSource::Current => "現在の設定".to_string(),
            BackupSource::Bak(n) => format!("バックアップ {n}"),
            BackupSource::PreUpgrade(version) => {
                format!("アップグレード前 (v{version})")
            }
        }
    }

    /// `<data_dir>` 直下のファイル名。
    pub fn filename(&self) -> String {
        match self {
            BackupSource::Current => "settings.db".to_string(),
            BackupSource::Bak(n) => format!("settings.db.bak{n}"),
            BackupSource::PreUpgrade(version) => {
                format!("settings.db.preupgrade-v{version}")
            }
        }
    }

    /// 「これは現状の settings.db そのもの」か。
    pub fn is_current(&self) -> bool {
        matches!(self, BackupSource::Current)
    }
}

/// 保存元バージョンと実行中バイナリの関係。
/// `RestoreCandidate` は最終的な deserialize 検証を復元直前にも行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupCompatibility {
    RestoreCandidate,
    NewerVersion,
    Unknown,
}

impl BackupCompatibility {
    pub fn can_attempt_restore(self) -> bool {
        self != Self::NewerVersion
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
    /// app_version による事前判定。実データの最終検証は restore_from でも必ず行う。
    pub compatibility: BackupCompatibility,
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
    /// 復元元 bak を `SettingsDb::open` + `load_into_settings` で開き直したが
    /// 内容が壊れていて読めない (= スキーマ違反 / JSON 破損 / etc.)。
    /// 復元してもアプリが立ち上がらないので、`restore_from` の冒頭で弾く。
    ValidationFailed(String),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestoreError::Io(e) => write!(f, "I/O エラー: {e}"),
            RestoreError::SourceMissing(p) => {
                write!(f, "復元元が見つかりません: {}", p.display())
            }
            RestoreError::ValidationFailed(msg) => {
                write!(f, "バックアップ内容を検証できませんでした: {msg}")
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

/// 失敗の重大度。UI 側で「閉じる」 (= recoverable) と「アプリを終了して再起動」
/// (= terminal) のどちらに倒すかの分類に使う (Codex P2 / 2026-05-17 対応)。
///
/// - `Recoverable`: まだ何もファイルを触っていない or 触ったが完全に rollback 済み。
///   `SAVE_SUPPRESSED` も巻き戻しているので、ユーザーはダイアログを閉じてアプリを
///   続けて使える。
/// - `Terminal`: `set_global_db(None)` で SQLite ハンドルを落としたあと、または
///   `settings.db` 関連を変更した後の失敗。`SAVE_SUPPRESSED` は立ったままで、
///   in-memory `Settings` も disk と乖離している。続けて操作すると挙動が壊れるので、
///   アプリを **強制終了** して再起動を促す。
#[derive(Debug)]
pub enum RestoreFailure {
    Recoverable(RestoreError),
    Terminal(RestoreError),
}

impl RestoreFailure {
    pub fn error(&self) -> &RestoreError {
        match self {
            RestoreFailure::Recoverable(e) | RestoreFailure::Terminal(e) => e,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, RestoreFailure::Terminal(_))
    }
}

impl std::fmt::Display for RestoreFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error().fmt(f)
    }
}

impl std::error::Error for RestoreFailure {}

// ──────────────────────────────────────────────────────────────────────
// 一覧
// ──────────────────────────────────────────────────────────────────────

/// `<data_dir>` 直下の現在 + bak1..bak10 + preupgrade snapshot を返す。
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
    let mut preupgrade = std::fs::read_dir(data_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = name.strip_prefix("settings.db.preupgrade-v")?;
            if version.is_empty() || !entry.path().is_file() {
                return None;
            }
            let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
            Some((modified, version.to_string(), entry.path()))
        })
        .collect::<Vec<_>>();
    preupgrade.sort_by_key(|(modified, _, _)| std::cmp::Reverse(*modified));
    for (_, version, path) in preupgrade {
        out.push(read_summary(BackupSource::PreUpgrade(version), &path));
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
        compatibility: BackupCompatibility::Unknown,
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
    // preupgrade 名の version は snapshot 作成時の last_seen_version から付けたもの。
    // 旧実装は DB open 時に app_version を現バイナリへ上書きしてから snapshot していたため、
    // 既存 preupgrade の schema_meta よりファイル名の方が正確。新実装では両者が一致する。
    if let BackupSource::PreUpgrade(version) = &summary.source {
        summary.app_version = Some(version.clone());
    }
    summary.compatibility = backup_compatibility(summary.app_version.as_deref());
    summary
}

fn backup_compatibility(app_version: Option<&str>) -> BackupCompatibility {
    let Some(saved) = app_version else {
        return BackupCompatibility::Unknown;
    };
    let Ok(saved) = semver::Version::parse(saved.trim_start_matches('v')) else {
        return BackupCompatibility::Unknown;
    };
    let Ok(current) = semver::Version::parse(env!("CARGO_PKG_VERSION")) else {
        return BackupCompatibility::Unknown;
    };
    if saved > current {
        BackupCompatibility::NewerVersion
    } else {
        BackupCompatibility::RestoreCandidate
    }
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
/// 副作用 (成功時):
/// - 現状の家族 (`settings.db` / `-wal` / `-shm` / `bak1..bak10`) を
///   `<name>.before-restore-<unix秒>` に **コピー** して退避 (= rename ではない、
///   元ファイルは温存)。
/// - `GLOBAL_DB` の Arc を落とし、稼働中の SQLite ハンドルを閉じる (= 直後の
///   atomic rename が Windows でロックされないようにする)。
/// - `SAVE_SUPPRESSED = true` を立てて、以降の `with_db` 経由 save を全部止める。
///
/// 失敗の分類 (Codex P2 / 2026-05-17 対応):
/// - `RestoreFailure::Recoverable`: validate / snapshot 失敗 (= まだファイルを触って
///   いない)。`SAVE_SUPPRESSED` を復元開始時の状態へ戻し、settings.db も無傷。
///   通常起動からならアプリを使い続けられるが、設定ブート保護中なら抑止を維持する。
/// - `RestoreFailure::Terminal`: `set_global_db(None)` 後 (= 既に SQLite ハンドルを
///   落とした) の失敗。`SAVE_SUPPRESSED` は立ったまま、in-memory `Settings` も disk と
///   乖離する。続けて操作すると挙動が崩れるので、UI 側で **強制終了 + 再起動** を促す。
///
/// **削除順** (Codex P1 / 2026-05-17 対応): WAL/SHM を **先** に削除してから
/// `settings.db` を atomic rename で差し替える。逆順だと「新 main + 旧 WAL」の混合
/// 状態で失敗 return → アプリが続行 → 次回起動時に SQLite が salt mismatch で
/// confused になる可能性がある。WAL/SHM 削除が失敗した場合は **main 未変更** なので
/// 退路がある (= 次回起動で同じ settings.db を再 open するだけ)。
pub fn restore_from(
    data_dir: &Path,
    source: &BackupSource,
) -> Result<RestoreReport, RestoreFailure> {
    // 設定ブート保護モーダルから呼ばれた場合は既に true。Recoverable 失敗時に
    // false へ固定すると、読み込めなかった設定 DB に既定値を書き戻せる状態へ
    // 変わってしまうため、呼び出し時の状態を保存して戻す。
    let save_suppressed_before_restore = crate::settings_db::save_suppressed();

    // 1. 検証: 復元元が `SettingsDb::open + load_into_settings` まで通るか。
    //    壊れた bak (= sqlite として開けるが load 段階で Corrupted になる、e.g.
    //    settings_kv JSON 破損 / UUID 不正 / 必須テーブル欠落) を選んで「復元
    //    完了 → 次回起動で fail」 になるのを防ぐ。Codex P3 (2026-05-17) 対応。
    validate_backup(data_dir, source).map_err(RestoreFailure::Recoverable)?;

    // 2. ここから抑止フラグを立てる。以降の with_db 経由の save は全部止まる。
    crate::settings_db::set_save_suppressed(true);

    // 3. 現状家族を退避。ここで失敗したら settings.db は無傷なので
    //    suppress を復元開始時の状態へ戻して Recoverable で return。
    let ts = unix_seconds_now();
    let snapshot_paths = match snapshot_current_family(data_dir, ts) {
        Ok(s) => s,
        Err(e) => {
            crate::settings_db::set_save_suppressed(save_suppressed_before_restore);
            return Err(RestoreFailure::Recoverable(e));
        }
    };

    // 4. **bak 復元** のときだけ、稼働中の SQLite で `PRAGMA wal_checkpoint(TRUNCATE)`
    //    を走らせて WAL の全 frame を main に統合する。これで `settings.db` 自体が
    //    self-contained になり、後段の rename / WAL 削除が失敗しても「未 checkpoint の
    //    WAL データが消える」事故を避けられる (Codex P2 / 2026-05-17 対応)。
    //
    //    `Current` (= WAL 破棄が user 意図) では checkpoint しない (= 破棄したい
    //    WAL データを main に統合してしまうと操作意図に反する)。
    //
    //    **checkpoint 失敗は strict 扱い** (Codex P1 round 3 / 2026-05-17): busy 戻り値
    //    も検査して「実は何も checkpoint されていない」状態を弾く。失敗時は何もファイル
    //    を触っていないので suppress を復元開始時の状態へ戻して Recoverable で return する。
    //    (旧版は best-effort + log だったが、checkpoint 不完全のまま WAL を削除すると
    //    user の直近設定が永久に失われる。)
    if !source.is_current() {
        if let Err(e) = crate::settings_db::checkpoint_global_db() {
            crate::settings_db::set_save_suppressed(save_suppressed_before_restore);
            return Err(RestoreFailure::Recoverable(RestoreError::ValidationFailed(
                format!("復元前の WAL チェックポイント (PRAGMA wal_checkpoint) に失敗: {e}"),
            )));
        }
    }

    // 5. GLOBAL_DB を None にして、生きている SQLite ハンドルを drop する。
    //    SettingsDb の Arc が他 (= 別スレッドの with_db closure 内) に
    //    クローンされている可能性があるが、SAVE_SUPPRESSED が立っているので
    //    新規の with_db は走らず、in-flight のは数 ms で終わる前提
    //    (本アプリの save は user 操作起点で長時間 hold しない)。万一残っていても
    //    後段の retry で吸収する。
    crate::settings_db::set_global_db(data_dir, None);

    // 6. **WAL/SHM を strict に削除** (Codex P1 round 3): 旧版は bak 復元時 best-effort
    //    だったが、`VACUUM INTO` で作った bak と旧 WAL の組み合わせでは salt mismatch
    //    が効かず、新 main + 旧 WAL の状態で SQLite が旧 WAL の行を **再適用** する
    //    ケースがある (= 復元が部分巻き戻る)。canonical 名から確実に外してから main
    //    rename に進む。
    //
    //    削除は SHM → WAL の順で行う。WAL は「未 checkpoint 分」を持つ本体なので
    //    最後に消し、途中失敗で Recoverable へ戻る場合に WAL 適用後の現行状態を残す。
    //    main は未変更なので失敗時は Recoverable: suppress を復元開始時の状態へ戻し、
    //    抑止されていなければ次回 with_db が lazy boot で同じ settings.db を再 open する
    //    (= checkpoint 済みなのでデータ無傷)。
    if let Err(e) = drop_wal_sidecars_strict(data_dir) {
        crate::settings_db::set_save_suppressed(save_suppressed_before_restore);
        return Err(RestoreFailure::Recoverable(e));
    }

    // 7. settings.db を atomic rename で差し替える (Current 選択時は no-op)。
    //    Current のときは「main を保ったまま WAL/SHM 破棄」が user 意図そのものなので、
    //    ここで何もせず success に倒す。
    //
    //    bak 復元の rename 失敗時: main 未変更 + checkpoint 済み + WAL/SHM 削除済み =
    //    user の元の設定は main にまるごと残っている状態。suppress を復元開始時の状態へ
    //    戻して Recoverable return する (抑止されていなければ次回 with_db lazy boot で
    //    main 再 open、データ無傷)。
    if !source.is_current() {
        let src_path = data_dir.join(source.filename());
        let main_path = data_dir.join("settings.db");
        if let Err(e) = replace_file_atomic_with_retry(&src_path, &main_path) {
            crate::settings_db::set_save_suppressed(save_suppressed_before_restore);
            return Err(RestoreFailure::Recoverable(e));
        }
    }

    Ok(RestoreReport { snapshot_paths })
}

/// 家族 (`settings.db` + `-wal` + `-shm` + `bak1..bak10`) をすべて削除する。
/// 削除前に同じ `before-restore-<ts>` パターンで退避 (= ユーザーが間違えて
/// 押した場合の救済材料を残す)。
///
/// **削除順** (Codex P1 / 2026-05-17 対応): `bak10..bak1` → `-shm` → `-wal` →
/// `settings.db` **最後**。途中失敗で settings.db が残れば次回起動が普通に通り、
/// ユーザーが再試行できる。逆順 (= main 最初) だと「main 不在 + family 残」状態で
/// 次回起動が `FailedFallbackDefault` (= 永久 save 抑止) に倒れる詰みパターンを作る。
///
/// 失敗の分類: `restore_from` と同じ。snapshot 前は `Recoverable`、`set_global_db(None)`
/// 後は `Terminal`。
///
/// 削除後、次回起動は `boot_settings_db_inner` で clean install 経路に入る。
pub fn full_reset(data_dir: &Path) -> Result<ResetReport, RestoreFailure> {
    let save_suppressed_before_reset = crate::settings_db::save_suppressed();
    crate::settings_db::set_save_suppressed(true);

    let ts = unix_seconds_now();
    let snapshot_paths = match snapshot_current_family(data_dir, ts) {
        Ok(s) => s,
        Err(e) => {
            crate::settings_db::set_save_suppressed(save_suppressed_before_reset);
            return Err(RestoreFailure::Recoverable(e));
        }
    };

    // GLOBAL_DB を落として SQLite ハンドルを閉じる (= remove_file がロック失敗
    // するのを避ける)。⚠️ ここから先の失敗はすべて Terminal。
    crate::settings_db::set_global_db(data_dir, None);

    let mut deleted: Vec<PathBuf> = Vec::new();
    for name in family_deletion_order_main_last() {
        let p = data_dir.join(&name);
        let res = retry_io(
            10,
            std::time::Duration::from_millis(50),
            || match std::fs::remove_file(&p) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e),
            },
        );
        match res {
            Ok(true) => deleted.push(p),
            Ok(false) => {}
            Err(e) => return Err(RestoreFailure::Terminal(RestoreError::Io(e))),
        }
    }

    Ok(ResetReport {
        snapshot_paths,
        deleted,
    })
}

/// `full_reset` 用の削除順。`settings.db` を **最後** にして、途中失敗時の
/// 「main 不在 + family 残」を防ぐ (Codex P1 / 2026-05-17 対応)。
fn family_deletion_order_main_last() -> Vec<String> {
    let mut v: Vec<String> = (1..=10)
        .rev()
        .map(|n| format!("settings.db.bak{n}"))
        .collect();
    v.push("settings.db-shm".to_string());
    v.push("settings.db-wal".to_string());
    v.push("settings.db".to_string());
    v
}

// ──────────────────────────────────────────────────────────────────────
// 内部ヘルパ
// ──────────────────────────────────────────────────────────────────────

const OPERATION_IMPORT_BACKUP_LIMIT: usize = 10;

/// Extract only operation customization from an older generation.
///
/// `Current` must come from the live in-memory `Settings` so its WAL-backed
/// state cannot be missed. Older generations are copied to a private temporary
/// directory before `SettingsDb` opens them.
pub fn load_operation_customize(
    data_dir: &Path,
    source: &BackupSource,
) -> Result<crate::operation_customize_share::OperationCustomizeBundle, RestoreError> {
    if source.is_current() {
        return Err(RestoreError::ValidationFailed(
            "current operation customization must be read from live settings".to_string(),
        ));
    }
    let src = data_dir.join(source.filename());
    if !src.exists() {
        return Err(RestoreError::SourceMissing(src));
    }
    // 一意な scratch dir を tempfile で確保する (旧実装は `pid + nanos` の手組み名)。
    // ランダム suffix + RAII cleanup で、直列化された復元テスト群が同名 dir を再利用して
    // Windows の delete-pending / sharing violation race を踏むのを防ぐ。SettingsDb の
    // ハンドルは load_operation_customize_in_dir 内で drop されてから TempDir が drop
    // される (= handle クローズが temp 削除より先)。
    let tmp = tempfile::Builder::new()
        .prefix("mimageviewer-operation-import-")
        .tempdir()
        .map_err(RestoreError::Io)?;
    load_operation_customize_in_dir(tmp.path(), &src)
}

fn load_operation_customize_in_dir(
    tmp_base: &Path,
    src: &Path,
) -> Result<crate::operation_customize_share::OperationCustomizeBundle, RestoreError> {
    let settings = load_settings_copy_in_dir(tmp_base, src)?;
    Ok(
        crate::operation_customize_share::OperationCustomizeBundle::from_settings(&settings)
            .with_label("バックアップ"),
    )
}

/// Save the live operation customization immediately before a replace import.
/// Only the newest bounded set is retained.
pub fn backup_operation_customize_before_import(
    data_dir: &Path,
    bundle: &crate::operation_customize_share::OperationCustomizeBundle,
) -> Result<PathBuf, RestoreError> {
    std::fs::create_dir_all(data_dir)?;
    let timestamp = unix_seconds_now();
    let mut ordinal = 1u32;
    let path = loop {
        let suffix = if ordinal == 1 {
            String::new()
        } else {
            format!("-{ordinal}")
        };
        let candidate = data_dir.join(format!(
            "operation-customize.before-import-{timestamp}{suffix}.mivkeys.json"
        ));
        if !candidate.exists() {
            break candidate;
        }
        ordinal += 1;
    };
    let json = crate::operation_customize_share::to_json(bundle)
        .map_err(|error| RestoreError::ValidationFailed(error.to_string()))?;
    std::fs::write(&path, json)?;
    prune_operation_import_backups(data_dir)?;
    Ok(path)
}

fn prune_operation_import_backups(data_dir: &Path) -> Result<(), RestoreError> {
    let mut backups: Vec<_> = std::fs::read_dir(data_dir)?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("operation-customize.before-import-")
                && name.ends_with(".mivkeys.json")
        })
        .collect();
    backups.sort_by_key(|entry| {
        std::cmp::Reverse(entry.metadata().and_then(|meta| meta.modified()).ok())
    });
    for entry in backups.into_iter().skip(OPERATION_IMPORT_BACKUP_LIMIT) {
        std::fs::remove_file(entry.path())?;
    }
    Ok(())
}

/// 選択した世代を temp dir に「settings.db」として展開し、`SettingsDb::open` +
/// `load_into_settings` まで通るか確認する。`Current` は実行中の DB 自身なので skip。
///
/// scratch dir は `tempfile` で一意名を確保する (`tempfile` は archive 変換で既に
/// production 依存に入っている)。旧実装は `pid + 秒` の手組み名だったため、同一秒に
/// 連続する復元呼び出し (= cargo test で直列化された復元テスト群) が同名 dir を
/// 再利用し、Windows の delete-pending / sharing violation race を誘発していた。
/// clean up は `TempDir` の Drop (= always-cleanup、失敗は無視) に任せる。
fn validate_backup(data_dir: &Path, source: &BackupSource) -> Result<(), RestoreError> {
    let src = data_dir.join(source.filename());
    if !src.exists() {
        return Err(RestoreError::SourceMissing(src));
    }
    if source.is_current() {
        // 実行中の `settings.db` は GLOBAL_DB で開かれているので、ここで
        // SettingsDb::open(tmp) しても自分自身の状態確認にならない。skip。
        return Ok(());
    }
    // SettingsDb ハンドルは validate_in_dir 内で drop されてから TempDir が drop される
    // (= handle クローズが temp 削除より先) 順序が保たれる。
    let tmp = tempfile::Builder::new()
        .prefix("mimageviewer-validate-")
        .tempdir()
        .map_err(RestoreError::Io)?;
    validate_in_dir(tmp.path(), &src)
}

fn validate_in_dir(tmp_base: &Path, src: &Path) -> Result<(), RestoreError> {
    load_settings_copy_in_dir(tmp_base, src).map(|_| ())
}

fn load_settings_copy_in_dir(
    tmp_base: &Path,
    src: &Path,
) -> Result<crate::settings::Settings, RestoreError> {
    let dst = tmp_base.join("settings.db");
    std::fs::copy(src, &dst).map_err(RestoreError::Io)?;
    match crate::settings_db::SettingsDb::open(tmp_base) {
        Ok(db) => db
            .load_into_settings()
            .map_err(|e| RestoreError::ValidationFailed(format!("読み込み失敗: {e}"))),
        Err(e) => Err(RestoreError::ValidationFailed(format!("open 失敗: {e}"))),
    }
}

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

/// `src` の内容で `dst` を atomic に置換する。`copy → rename` パターン:
/// 1. `dst.restoring-tmp` に `src` を copy
/// 2. `dst.restoring-tmp` を `dst` に rename (Windows / Unix とも atomic)
///
/// SQLite ハンドルが直前まで `dst` を握っていた場合、稀に rename が短時間
/// ロック失敗する可能性があるので、50ms x 10 回まで retry する。
fn replace_file_atomic_with_retry(src: &Path, dst: &Path) -> Result<(), RestoreError> {
    let tmp = with_suffix(dst, ".restoring-tmp");
    if let Err(e) = std::fs::copy(src, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(RestoreError::Io(e));
    }
    let rename_result = retry_io(10, std::time::Duration::from_millis(50), || {
        std::fs::rename(&tmp, dst)
    });
    match rename_result {
        Ok(()) => Ok(()),
        Err(e) => {
            // rename 失敗 → tmp を掃除。`dst` は無傷 (rename は atomic)。
            let _ = std::fs::remove_file(&tmp);
            Err(RestoreError::Io(e))
        }
    }
}

/// WAL/SHM を retry 付きで削除する。NotFound は成功扱い。最後まで残ったら error。
///
/// 2026-05-17 Codex P1 round 3 対応: 旧版で bak 復元時に best-effort 削除に倒していた
/// 経路は廃止した。理由は、`VACUUM INTO` で作った bak と旧 WAL の組み合わせでは WAL
/// salt mismatch ガードが効かず、SQLite が旧 WAL の行を新 main に再適用しうるため。
/// canonical 名から確実に削除できたことを担保してから rename に進む。
fn drop_wal_sidecars_strict(data_dir: &Path) -> Result<(), RestoreError> {
    for name in sidecar_deletion_order_wal_last() {
        let p = data_dir.join(name);
        let result =
            retry_io(
                10,
                std::time::Duration::from_millis(50),
                || match std::fs::remove_file(&p) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e),
                },
            );
        if let Err(e) = result {
            return Err(RestoreError::Io(e));
        }
    }
    Ok(())
}

fn sidecar_deletion_order_wal_last() -> [&'static str; 2] {
    ["settings.db-shm", "settings.db-wal"]
}

fn retry_io<F, T>(
    max_attempts: usize,
    delay: std::time::Duration,
    mut op: F,
) -> Result<T, std::io::Error>
where
    F: FnMut() -> Result<T, std::io::Error>,
{
    debug_assert!(max_attempts > 0);
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..max_attempts {
        if attempt > 0 {
            std::thread::sleep(delay);
        }
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.expect("max_attempts > 0 なので必ず 1 度は op が走る"))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
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

    /// 並列 `cargo test` のもとでは、AV / Windows Search インデクサ / 直前に閉じた
    /// SQLite ハンドルが settings.db 家族ファイルを一瞬ロックし、`restore_from` /
    /// `full_reset` の snapshot 前に起きた transient な `Recoverable(Io)` も同様に再試行
    /// できる。削除開始後の失敗は `Terminal` であり、部分削除済みの可能性があるため
    /// この helper では再試行しない。
    ///
    /// `Recoverable` は「まだファイルを変更していない = 安全に再試行できる」契約なので
    /// (`restore_from` の失敗は全て `Recoverable`。`full_reset` は削除前の snapshot 失敗のみ
    /// `Recoverable`)、テスト側では **transient な IO 失敗に限り** リトライしてロックが
    /// 解けるのを待つ。`Terminal` や `ValidationFailed` のような恒久エラーは即座に返し、
    /// 本物のバグを握り潰さない。
    fn retry_on_transient_lock<T>(
        mut op: impl FnMut() -> Result<T, RestoreFailure>,
    ) -> Result<T, RestoreFailure> {
        // 最大 40 回リトライ (× 50ms = 2s)。並列テストの transient lock は通常 1s 未満で解ける。
        let mut result = op();
        let mut retries = 0;
        while retries < 40 {
            match &result {
                Err(f) if !f.is_terminal() && matches!(f.error(), RestoreError::Io(_)) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    result = op();
                    retries += 1;
                }
                _ => break,
            }
        }
        result
    }

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
        assert_eq!(
            summaries[1].app_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            summaries[1].compatibility,
            BackupCompatibility::RestoreCandidate
        );
    }

    #[test]
    fn list_backups_includes_preupgrade_snapshot_with_saved_version() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&Settings::default()).unwrap();
            db.backup_to(&dir.join("settings.db.preupgrade-v2.4.1"))
                .unwrap();
        }

        let summaries = list_backups(dir);
        let preupgrade = summaries
            .iter()
            .find(|summary| {
                matches!(
                    &summary.source,
                    BackupSource::PreUpgrade(version) if version == "2.4.1"
                )
            })
            .expect("preupgrade snapshot must be listed");
        assert_eq!(preupgrade.app_version.as_deref(), Some("2.4.1"));
    }

    #[test]
    fn list_backups_marks_newer_saved_version_unavailable() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&Settings::default()).unwrap();
            db.backup_to(&dir.join("settings.db.bak1")).unwrap();
        }
        let conn = Connection::open(dir.join("settings.db.bak1")).unwrap();
        conn.execute(
            "UPDATE schema_meta SET value = '999.0.0' WHERE key = 'app_version'",
            [],
        )
        .unwrap();
        drop(conn);

        let summaries = list_backups(dir);
        let newer = summaries
            .iter()
            .find(|summary| summary.source == BackupSource::Bak(1))
            .unwrap();
        assert_eq!(newer.app_version.as_deref(), Some("999.0.0"));
        assert_eq!(newer.compatibility, BackupCompatibility::NewerVersion);
        assert!(!newer.compatibility.can_attempt_restore());
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
        let report = retry_on_transient_lock(|| restore_from(dir, &BackupSource::Bak(1))).unwrap();
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
        // This test owns the family-file deletion/snapshot contract, not SQLite's
        // VACUUM INTO behavior (covered by SettingsDb tests). Plain closed files keep
        // it independent from AV/indexer locks on freshly-created SQLite snapshots
        // during parallel `cargo test` runs on Windows.
        std::fs::write(dir.join("settings.db"), b"main").unwrap();
        std::fs::write(dir.join("settings.db.bak1"), b"bak1").unwrap();
        std::fs::write(dir.join("settings.db.bak2"), b"bak2").unwrap();
        let report = retry_on_transient_lock(|| full_reset(dir)).unwrap();
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

    /// 2026-05-17 Codex P1 対応の回帰テスト: 稼働中の `GLOBAL_DB` (= SQLite ハンドル
    /// が settings.db を握っている) の状態で復元しても、ロックエラーで失敗せず
    /// settings.db が正しく差し替わること。
    #[test]
    fn restore_from_works_with_live_global_db_handle() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        let good_state = sample_settings();
        let bad_state = Settings::default();
        // good_state を bak1 に取り、settings.db には bad_state を書いた状態を作る。
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&good_state).unwrap();
            db.backup_to(&dir.join("settings.db.bak1")).unwrap();
            db.save_full(&bad_state).unwrap();
        }
        // `boot_settings_db` で GLOBAL_DB を populate する (= 実アプリと同じ状態)。
        let outcome = crate::settings_db::boot_settings_db(dir);
        assert!(outcome.db.is_some(), "GLOBAL_DB に Arc がセットされる");
        assert_eq!(outcome.settings.favorites.len(), bad_state.favorites.len());
        // outcome 自体は drop するが、GLOBAL_DB が Arc を保持しているので
        // SQLite ハンドルは生きたまま。restore_from が `set_global_db(None)` で
        // 確実に閉じてからファイル操作する経路を踏む。
        drop(outcome);

        let report = retry_on_transient_lock(|| restore_from(dir, &BackupSource::Bak(1)))
            .expect("生きた GLOBAL_DB の状態でも復元は成功する");
        assert!(!report.snapshot_paths.is_empty());

        // WAL/SHM は確実に消えていること (Codex P1: "破棄できたことを保証")。
        assert!(!dir.join("settings.db-wal").exists());
        assert!(!dir.join("settings.db-shm").exists());

        // settings.db は good_state の内容になっている。
        let db = SettingsDb::open(dir).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_eq!(loaded.favorites.len(), good_state.favorites.len());
    }

    /// 2026-05-17 Codex P3 対応: 壊れた bak (= sqlite として開けるが
    /// load_into_settings で Corrupted になる) を選んだら、`ValidationFailed` で
    /// 失敗して settings.db は **無傷のまま**、save 抑止を呼び出し時から変えないこと。
    #[test]
    fn restore_from_corrupted_bak_fails_validation_and_preserves_state() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        // 正常な settings.db を用意。
        let original = sample_settings();
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&original).unwrap();
            // WAL を main に統合して settings.db を self-contained にする。この経路の
            // restore_from(Bak) は validate 失敗で main を **書き換えない** ので、末尾の
            // 再 open は「setup で書いた main が close-on-drop の checkpoint で永続化された」
            // ことに依存する。並列テストの高負荷下では AV / インデクサが -wal / -shm を
            // 一瞬握って close 時 checkpoint が不完全になり、再 open が 0 favorites を
            // 読む race がある (2026-07-12 flake)。明示 checkpoint で決定的にする。
            db.checkpoint_truncate().unwrap();
        }
        // bak1 として「sqlite ヘッダではない」16 バイトを書く (= open 段階で Corrupted)。
        std::fs::write(dir.join("settings.db.bak1"), b"NOT A SQLITE DB!").unwrap();

        let save_suppressed_before_restore = crate::settings_db::save_suppressed();
        let failure = restore_from(dir, &BackupSource::Bak(1))
            .expect_err("壊れた bak は validate で弾かれる");
        // Recoverable で、内訳は ValidationFailed (= 何もファイルを触っていない失敗)。
        assert!(
            !failure.is_terminal(),
            "validate 失敗は Recoverable (= アプリ続行可) であるべき"
        );
        assert!(
            matches!(failure.error(), RestoreError::ValidationFailed(_)),
            "expected ValidationFailed, got: {failure:?}"
        );

        // settings.db は無傷で original を読み返せる。
        let db = SettingsDb::open(dir).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_eq!(loaded.favorites.len(), original.favorites.len());

        // 通常起動では save 抑止を新たに立てない。
        assert_eq!(
            crate::settings_db::save_suppressed(),
            save_suppressed_before_restore,
            "validate 失敗は呼び出し時の save 抑止状態を変えてはならない"
        );

        // 非互換 / 読み込み失敗のブート保護中から復元を試した場合は、失敗後も
        // 抑止を解除しない。既定設定が unreadable な DB を上書きするのを防ぐ。
        crate::settings_db::set_save_suppressed(true);
        let failure = restore_from(dir, &BackupSource::Bak(1))
            .expect_err("ブート保護中でも壊れた bak は validate で弾かれる");
        assert!(!failure.is_terminal());
        assert!(
            crate::settings_db::save_suppressed(),
            "ブート保護中の復元失敗で save 抑止を解除してはならない"
        );
    }

    /// 2026-05-17 Codex P2 対応: bak 復元の冒頭で `wal_checkpoint(TRUNCATE)` を走らせて
    /// `settings.db` を self-contained にする。これで rename 失敗時に「未 checkpoint の
    /// WAL データが消える」事故を避けられる。
    ///
    /// 直接テストできるのは「Bak 復元成功後に WAL が残っていない」+「復元後の main
    /// に最新データがある」の組合せで、checkpoint が走ったことの間接的な観測になる。
    #[test]
    fn restore_from_bak_checkpoints_wal_before_swap() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();

        // bak1 に "古い state" を用意。
        let old_state = {
            let mut s = Settings::default();
            s.add_favorite("old".into(), std::path::PathBuf::from(r"C:\old"));
            s
        };
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&old_state).unwrap();
            db.backup_to(&dir.join("settings.db.bak1")).unwrap();
        }
        // 実 app 状態を再現: boot で GLOBAL_DB を populate し、追加 save を走らせて
        // WAL に未 checkpoint frame を作る。
        let outcome = crate::settings_db::boot_settings_db(dir);
        assert!(outcome.db.is_some());
        // 何回か save を回す。これで WAL に frame が積まれる (sqlite のデフォルト
        // auto-checkpoint 閾値 = 1000 page / 約 4MB なので、数回の save では発火しない)。
        let mut newer = old_state.clone();
        for i in 0..5 {
            newer.add_favorite(format!("new{i}"), std::path::PathBuf::from("C:\\new"));
            crate::settings_db::with_db_result(|db| db.save_full(&newer)).unwrap();
        }
        // 本番では `GLOBAL_DB` が Arc の唯一の所有者なので、`outcome.db` の clone を
        // drop して同じ状況を作る。これを残すと `set_global_db(None)` が Arc を落とし
        // きれず、Windows で rename が PermissionDenied になる。
        drop(outcome);

        // 復元実行 (bak1 = 古い state へ戻す)。
        // 復元成功 = checkpoint も無事に走ったとみなす (= 失敗してもベストエフォートで
        // 続行するが、ここでは本番想定のスムーズな成功経路を見たい)。
        let report = retry_on_transient_lock(|| restore_from(dir, &BackupSource::Bak(1)))
            .expect("checkpoint + 復元は成功する");
        assert!(!report.snapshot_paths.is_empty());

        // 復元後の WAL は最終的に消える (bak 復元の best-effort delete 経路でも、
        // 同プロセス内ならハンドルが drop 済みで削除が通る前提)。
        assert!(!dir.join("settings.db-wal").exists());

        // 復元後の main = bak1 = old_state。
        let db = SettingsDb::open(dir).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_eq!(loaded.favorites.len(), old_state.favorites.len());
    }

    /// 2026-05-17 Codex P1 対応: `full_reset` の削除順は **main 最後**。
    /// 万一途中で失敗しても settings.db が残るので、次回起動が `FailedFallbackDefault`
    /// に倒れない (= 永久 save 抑止の詰みパターンを防ぐ)。
    #[test]
    fn full_reset_deletion_order_keeps_main_last() {
        let order = family_deletion_order_main_last();
        // 末尾は必ず settings.db。
        assert_eq!(order.last().map(String::as_str), Some("settings.db"));
        // main は 1 度だけ登場 (= 順序リストに重複なし)。
        let main_count = order.iter().filter(|n| n.as_str() == "settings.db").count();
        assert_eq!(main_count, 1);
        // bak10 は main より前。
        let bak10_idx = order.iter().position(|n| n == "settings.db.bak10").unwrap();
        let main_idx = order.iter().position(|n| n == "settings.db").unwrap();
        assert!(bak10_idx < main_idx, "bak10 は main より前に消す");
        // -wal / -shm も main より前。
        let wal_idx = order.iter().position(|n| n == "settings.db-wal").unwrap();
        let shm_idx = order.iter().position(|n| n == "settings.db-shm").unwrap();
        assert!(wal_idx < main_idx);
        assert!(shm_idx < main_idx);
    }

    /// Current 復元では WAL 破棄がデータ破棄そのものなので、WAL は最後に消す。
    /// SHM 削除で失敗した場合は WAL が残り、Recoverable として現行状態へ戻れる。
    #[test]
    fn sidecar_deletion_order_keeps_wal_last() {
        let order = sidecar_deletion_order_wal_last();
        assert_eq!(order, ["settings.db-shm", "settings.db-wal"]);
    }

    /// `current` を選んだケースでは bak の中身に依存しないので validate は skip。
    /// WAL/SHM の破棄だけ走り、settings.db は無傷で残る。
    #[test]
    fn restore_from_current_drops_wal_but_keeps_main() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        let state = sample_settings();
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&state).unwrap();
            // まず main へ checkpoint して「無傷であるべき good state」を self-contained に
            // 永続化する。これで末尾の再 open は close-on-drop の checkpoint 成否 (並列高負荷下で
            // AV / インデクサの -wal/-shm ロックにより不完全化しうる) に依存しなくなる。
            db.checkpoint_truncate().unwrap();
            // その後、何回か save を回して WAL に (Current 復元が破棄する) frame を積む。
            // main は既に durable なので、この WAL が checkpoint されなくても再 open は無事。
            for _ in 0..3 {
                db.save_full(&state).unwrap();
            }
        }
        // WAL がディスクに残るのは sqlite の checkpoint タイミング次第なので、
        // ここでは「WAL/SHM が消えていること」 + 「main が無傷」だけを確認する。
        let report = retry_on_transient_lock(|| restore_from(dir, &BackupSource::Current)).unwrap();
        assert!(!dir.join("settings.db-wal").exists());
        assert!(!dir.join("settings.db-shm").exists());
        let db = SettingsDb::open(dir).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_eq!(loaded.favorites.len(), state.favorites.len());
        let _ = report;
    }

    #[test]
    fn load_operation_customize_extracts_three_fields_from_generation() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        let mut expected = sample_settings();
        expected.keymap.overrides = vec![crate::keymap::KeyBindingOverride {
            action: "GridPin".to_string(),
            chords: vec!["Ctrl+P".to_string()],
        }];
        expected.ring_shortcuts.mouse_ring_help_visible = false;
        expected.menu_layout.hidden_commands = vec!["SettingsOperationCustomize".to_string()];
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&expected).unwrap();
            db.backup_to(&dir.join("settings.db.bak1")).unwrap();
            db.save_full(&Settings::default()).unwrap();
        }

        let bundle = load_operation_customize(dir, &BackupSource::Bak(1)).unwrap();
        assert_eq!(bundle.keymap, expected.keymap);
        assert_eq!(bundle.ring_shortcuts, expected.ring_shortcuts);
        assert_eq!(bundle.menu_layout, expected.menu_layout);
    }

    #[test]
    fn load_operation_customize_rejects_corrupted_generation_without_touching_main() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        let expected = sample_settings();
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&expected).unwrap();
            // WAL を main に統合して settings.db を self-contained にする。load_operation_customize
            // は main を **一切触らない** (壊れた bak を scratch dir に copy して検証するだけ) ので、
            // 末尾の再 open は setup の main 永続化に依存する。並列テストの高負荷下では
            // close-on-drop の checkpoint が AV / インデクサの -wal/-shm 一時ロックで不完全になり、
            // 再 open が 0 favorites を読む race がある (2026-07-12 flake)。明示 checkpoint で決定的にする。
            db.checkpoint_truncate().unwrap();
        }
        std::fs::write(dir.join("settings.db.bak1"), b"NOT A SQLITE DB!").unwrap();

        let error = load_operation_customize(dir, &BackupSource::Bak(1)).unwrap_err();
        assert!(matches!(error, RestoreError::ValidationFailed(_)));
        let db = SettingsDb::open(dir).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_eq!(loaded.favorites.len(), expected.favorites.len());
    }

    #[test]
    fn before_import_backup_is_valid_bundle_and_retention_is_bounded() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        let bundle = crate::operation_customize_share::OperationCustomizeBundle::from_settings(
            &sample_settings(),
        );
        let first = backup_operation_customize_before_import(dir, &bundle).unwrap();
        let text = std::fs::read_to_string(first).unwrap();
        assert_eq!(
            crate::operation_customize_share::parse_json(&text)
                .unwrap()
                .bundle
                .keymap,
            bundle.keymap
        );

        for _ in 0..OPERATION_IMPORT_BACKUP_LIMIT + 2 {
            backup_operation_customize_before_import(dir, &bundle).unwrap();
        }
        let count = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("operation-customize.before-import-")
            })
            .count();
        assert_eq!(count, OPERATION_IMPORT_BACKUP_LIMIT);
    }

    fn sample_settings() -> Settings {
        let mut s = Settings::default();
        s.add_favorite("a".into(), std::path::PathBuf::from(r"C:\a"));
        s.add_favorite("b".into(), std::path::PathBuf::from(r"C:\b"));
        s
    }
}
