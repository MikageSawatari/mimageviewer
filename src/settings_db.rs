//! 設定永続化 SQLite バックエンド (Phase 1)。
//!
//! `%APPDATA%/mimageviewer/settings.db` に `Settings` を保存する。詳細設計は
//! [docs/settings-sqlite-migration.md](../../docs/settings-sqlite-migration.md)
//! を参照。
//!
//! ## ラウンドトリップ戦略
//!
//! `Settings` 構造体には 80+ フィールドあり、手で全部 SQL カラムに展開するのは
//! メンテ不能。そこで:
//!
//! 1. `serde_json::to_value(&settings)` で全フィールドを `Value::Object` に変換
//! 2. **複合フィールド** (`favorites` / `tags` / `video_resume_positions` /
//!    `vst3_plugins` / `vst3_chain_slots` / `recent_open_with_apps` /
//!    `custom_open_with_apps`) は別テーブルに切り出してから Map から remove
//! 3. 残り全部を `settings_kv (key, value)` に JSON 値そのままで格納
//!
//! ロード時は逆に複合テーブル → typed struct → JSON Value → Map に挿入してから
//! `serde_json::from_value::<Settings>(Map)` で復元する。
//!
//! これにより:
//! - `Settings` に新フィールドが追加されても schema を変えずに自動的に永続化される
//! - 既存の `serde(default)` 属性が「DB 側で欠落していたらデフォルト値」として
//!   そのまま機能する (= 旧 DB を新コードで開いても安全)
//! - 複合テーブルは hash skip (VST3) や hot-path upsert (video_resume_positions)
//!   などの最適化を別個に適用できる

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::settings::{
    FavoriteEntry, RecentApp, Settings, TagDef, Vst3ChainPresetSlot, Vst3ChainPresetSlots,
    Vst3PluginEntry,
};

const SCHEMA_VERSION: &str = "1";

/// `settings_kv` に格納する**しない** (= 専用テーブルに切り出される) フィールド名一覧。
/// `Settings` の serde フィールド名と一致させる。
///
/// ⚠️ **将来このリストを増やす場合の migration 注意点** (Codex P3 2026-05-13):
/// 既存環境ではこのキーが `settings_kv` に旧 JSON のままで残っている。`build_settings_from_db`
/// は `read_settings_kv` で map にそれを取り込んだ後、空の新テーブル由来の値で
/// **上書きしてしまう** (`map.insert("vst3_plugins", ...)` が既存値を破棄するため)。
///
/// 新しい complex field をこのリストに追加するときは:
///   1. 同じ load パス内で、新テーブルが空のときだけ legacy JSON を残す分岐を入れる、
///   2. または起動時に一度限りの `settings_kv → 新テーブル` migration step を追加し、
///      完了後 `settings_kv` 側の row を削除する、
///
/// のどちらかを行うこと。**何もせず追加すると初回起動でユーザー設定が消える。**
const COMPLEX_FIELDS: &[&str] = &[
    "favorites",
    "tags",
    "video_resume_positions",
    "vst3_plugins",
    "vst3_chain_slots",
    "recent_open_with_apps",
    "custom_open_with_apps",
];

// ---------------------------------------------------------------------------
// SettingsDbError
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum SettingsDbError {
    /// SQLite open / I/O 系の transient エラー (= 再起動で直る可能性あり)。
    /// 呼び出し側は `MAIN_UNREADABLE_THIS_SESSION` を立てて save を抑止する。
    Transient(rusqlite::Error),
    /// DB ファイルが壊れている (NotADatabase / DatabaseCorrupt / integrity_check 失敗)。
    /// 呼び出し側は `.corrupted-<ts>` に quarantine してから bak を試行する。
    Corrupted(String),
    /// 権限エラー (% APPDATA% が ReadOnly 等)。
    Permission(rusqlite::Error),
    /// `create_new` が呼ばれたが既に bootstrap 済み DB が存在する状態
    /// (Codex P2 v7 2026-05-13)。Phase 2 の decision tree が family を transient で
    /// 見逃して clean install 経路に倒れたケースを検出する。**この変種を受けた呼び出し側は
    /// `save_full` を呼んではならない** (= ユーザー設定の上書きを防ぐ)。`open()` で
    /// 開き直すか、上層で別の fallback を試す。
    AlreadyBootstrapped,
    /// 本セッションの save が抑止されている (Codex P2 v8b-3 2026-05-14)。
    /// `boot_settings_db` が FailedFallbackDefault を返したあとなど。`with_db` が
    /// この variant を返すと、Phase 3 caller は `save_full` を呼ばずに skip する。
    SaveSuppressed,
    /// その他の rusqlite エラー。
    Rusqlite(rusqlite::Error),
    /// JSON ラウンドトリップ失敗。
    Serde(serde_json::Error),
    /// `Mutex` poison。スレッドが panic した状態。
    Poisoned,
}

impl std::fmt::Display for SettingsDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(e) => write!(f, "transient sqlite error: {e}"),
            Self::Corrupted(msg) => write!(f, "settings.db corrupted: {msg}"),
            Self::Permission(e) => write!(f, "permission denied: {e}"),
            Self::AlreadyBootstrapped => {
                write!(f, "create_new called on already-bootstrapped settings.db")
            }
            Self::SaveSuppressed => write!(f, "save suppressed this session"),
            Self::Rusqlite(e) => write!(f, "sqlite error: {e}"),
            Self::Serde(e) => write!(f, "serde_json error: {e}"),
            Self::Poisoned => write!(f, "settings db mutex poisoned"),
        }
    }
}

impl std::error::Error for SettingsDbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transient(e) | Self::Permission(e) | Self::Rusqlite(e) => Some(e),
            Self::Serde(e) => Some(e),
            Self::Corrupted(_)
            | Self::Poisoned
            | Self::AlreadyBootstrapped
            | Self::SaveSuppressed => None,
        }
    }
}

impl From<rusqlite::Error> for SettingsDbError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Rusqlite(e)
    }
}

impl From<serde_json::Error> for SettingsDbError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e)
    }
}

/// SQLite open エラーの分類 (spec §5.1)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFailureKind {
    /// DB が物理的に壊れている。Corrupted / NotADatabase / integrity_check 失敗。
    /// quarantine 対象。
    Corrupted,
    /// 一時的な I/O 失敗。AV / cloud sync / busy 等。retry 後にも続けば save 抑止。
    Transient,
    /// ファイル権限・read-only。
    Permission,
    /// 分類不能。Transient と同じ扱いだが telemetry 用に区別。
    Other,
}

pub fn classify_open_error(e: &rusqlite::Error) -> OpenFailureKind {
    use rusqlite::ErrorCode::*;
    let err = match e {
        rusqlite::Error::SqliteFailure(err, _) => err,
        _ => return OpenFailureKind::Other,
    };
    match err.code {
        NotADatabase | DatabaseCorrupt => OpenFailureKind::Corrupted,
        DatabaseBusy | DatabaseLocked | CannotOpen | SystemIoFailure => OpenFailureKind::Transient,
        PermissionDenied | ReadOnly => OpenFailureKind::Permission,
        _ => OpenFailureKind::Other,
    }
}

/// `rusqlite::Error` を `SettingsDbError` に変換する。
///
/// spec §5.1 に従い、open 経路 (open + PRAGMA + integrity_check + init_schema) の
/// **どの段階で出たエラーでも** 一貫して分類する。Codex P1 2026-05-13 への対応。
///
/// 同時に primary code と extended_code を `settings_diag_log` に出力する
/// (= `SystemIoFailure` などの内訳を後追いで特定可能にするため、spec §5.1 / P3 対応)。
fn classify_rusqlite_error_for_open(e: rusqlite::Error, where_: &str) -> SettingsDbError {
    if let rusqlite::Error::SqliteFailure(err, _) = &e {
        log_diag(&format!(
            "settings_db: {where_} sqlite error: primary={:?} extended={}",
            err.code, err.extended_code
        ));
    } else {
        log_diag(&format!("settings_db: {where_} error: {e}"));
    }
    match classify_open_error(&e) {
        OpenFailureKind::Corrupted => SettingsDbError::Corrupted(format!("{where_}: {e}")),
        OpenFailureKind::Permission => SettingsDbError::Permission(e),
        OpenFailureKind::Transient => SettingsDbError::Transient(e),
        OpenFailureKind::Other => SettingsDbError::Transient(e),
    }
}

/// open エラーが `Transient` 分類かどうか (= retry 候補か)。
fn is_transient_error(e: &SettingsDbError) -> bool {
    matches!(e, SettingsDbError::Transient(_))
}

/// 診断ログヘルパ。`crate::settings::settings_diag_log` は private なので、
/// 同等の append-only sink にここから書く (= `<data_dir>/logs/settings.log`)。
///
/// settings.rs の関数を pub にすると lib との二重定義になる + Phase 6 で
/// 削除予定の経路が増えるため、ここでは独立した実装を持つ (両者が同じ
/// settings.log に append するのは意図的)。
fn log_diag(msg: &str) {
    use std::io::Write;
    crate::logger::log(msg);
    let path = crate::data_dir::logs_dir().join("settings.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{timestamp}] {msg}\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

// ---------------------------------------------------------------------------
// SettingsDb
// ---------------------------------------------------------------------------

/// 設定永続化 DB ハンドル。
///
/// `Arc<SettingsDb>` で複数スレッドから共有する。内部の `Mutex<Inner>` が
/// 直列化を担い、`with_db` 経由でアクセスする。
pub struct SettingsDb {
    inner: Mutex<Inner>,
}

struct Inner {
    conn: Connection,
    /// VST3 chain (= `Settings::vst3_plugins`) の **直近 commit 成功時** の hash。
    /// `None` = 未確認 (= 起動直後 / load 前)。`Some(h)` = h と一致するなら DB と
    /// in-memory は同じ内容なので DELETE+INSERT をスキップする。
    last_saved_vst3_chain_hash: Option<u64>,
    /// VST3 chain slots (preset 10 個) も同様。
    last_saved_vst3_slots_hash: Option<u64>,
}

/// Transient 失敗時の retry 回数 + 間隔 (spec §5)。
///
/// spec の「50ms backoff で最大 3 回 retry」を厳格に解釈し、
/// **初回 + 3 retries = 計 4 attempts、間に 50ms sleep を最大 3 回** とする
/// (Codex P2 2026-05-13: 元の 3 attempts/2 sleeps は緩い解釈だったため引き上げ)。
const OPEN_RETRY_ATTEMPTS: u32 = 4;
const OPEN_RETRY_BACKOFF_MS: u64 = 50;

/// `SettingsDb::open` / `create_new` で使う open mode 選択 (Codex P2 v3 2026-05-13)。
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenMode {
    /// 既存の main DB を開く。`SQLITE_OPEN_CREATE` を **付けない** ことで、
    /// AV / cloud sync 等で一時的にファイルが消えても新規 DB が作られず、
    /// `CannotOpen` → `Transient` として上層に伝わる。spec §5 の
    /// 「family が見える → open existing」経路で使う想定。
    RequireExisting,
    /// 新規 DB を作成する (= bootstrap)。クリーンインストール / JSON migration 直後 /
    /// quarantine 後の fresh-init で使う。Phase 2 から呼ばれる。
    CreateNew,
}

impl SettingsDb {
    /// `<data_dir>/settings.db` を **既存ファイル前提で** 開く。
    ///
    /// `Transient` 分類のエラーは spec §5 に従って `OPEN_RETRY_ATTEMPTS` 回まで
    /// 自動 retry する。`Corrupted` / `Permission` は即座に返す
    /// (= 環境ステータスが変わらないと直らない)。
    ///
    /// `SQLITE_OPEN_CREATE` を **付けない** ので、ファイルが (transient かつ) 不在
    /// なら `CannotOpen` → `Transient` が上層に返る。空 DB の自動作成でユーザー設定が
    /// defaults に上書きされる事故 (= 今回の SQLite 移行の主動機) を構造的に防ぐ
    /// (Codex P2 v3 2026-05-13)。
    ///
    /// クリーンインストールや migration 直後の fresh-init には [`create_new`] を使うこと。
    pub fn open(data_dir: &Path) -> Result<Self, SettingsDbError> {
        Self::open_with_mode(data_dir, OpenMode::RequireExisting)
    }

    /// `<data_dir>/settings.db` を新規作成して開く (= bootstrap)。
    ///
    /// `SQLITE_OPEN_CREATE` フラグを付ける。ファイルが存在しなければ作成し、
    /// 既存 bootstrapped DB ならエラー (`AlreadyBootstrapped`) を返す。
    /// Phase 2 のクリーンインストール / JSON migration 直後 / quarantine 後の
    /// fresh-init で使う。retry セマンティクスは [`open`] と同じ。
    ///
    /// **2 段の safety net** (Codex P2 v7 + v8 2026-05-13):
    /// 1. 呼び出し直前に `settings_db_family_exists()` を確認 — family の **どれかが**
    ///    可視なら、Phase 2 が transient な family-miss-detect で誤って clean-install
    ///    経路を選んでいる疑いが強いので、`AlreadyBootstrapped` で fail-fast する。
    ///    これで bak chain が孤立して救えなくなる事故を防ぐ。
    /// 2. main DB を CREATE フラグ込みで開いた後、`bootstrap_complete` row が
    ///    既に存在すれば `AlreadyBootstrapped` (= 既存ユーザー設定を握っている DB を
    ///    save_full(Default::default()) で消すのを防ぐ)。
    pub fn create_new(data_dir: &Path) -> Result<Self, SettingsDbError> {
        // Codex P2 v8: open より前に family pre-check。bak / WAL / SHM が見えるなら
        // (= "clean install" を選んだ前提が崩れているなら) 即座に AlreadyBootstrapped で
        // 失敗し、上層は bak / 旧 JSON への fallback を試みる。
        if settings_db_family_exists(data_dir) {
            log_diag(
                "settings_db: create_new pre-check: family is visible; \
                 refusing to clobber existing setup (Codex P2 v8)",
            );
            return Err(SettingsDbError::AlreadyBootstrapped);
        }
        Self::open_with_mode(data_dir, OpenMode::CreateNew)
    }

    fn open_with_mode(data_dir: &Path, mode: OpenMode) -> Result<Self, SettingsDbError> {
        let path = settings_db_path(data_dir);
        if let Some(parent) = path.parent() {
            // dir 作成失敗は無視 (= conn open でも同じエラーが出るのでそちらに任せる)。
            let _ = std::fs::create_dir_all(parent);
        }

        let mut last_err: Option<SettingsDbError> = None;
        for attempt in 0..OPEN_RETRY_ATTEMPTS {
            match Self::try_open_once(&path, mode) {
                Ok(db) => {
                    if attempt > 0 {
                        log_diag(&format!(
                            "settings_db: open succeeded on attempt {} after transient failure",
                            attempt + 1
                        ));
                    }
                    return Ok(db);
                }
                Err(e) if is_transient_error(&e) => {
                    log_diag(&format!(
                        "settings_db: open attempt {} failed (transient): {e}",
                        attempt + 1
                    ));
                    last_err = Some(e);
                    if attempt + 1 < OPEN_RETRY_ATTEMPTS {
                        std::thread::sleep(std::time::Duration::from_millis(OPEN_RETRY_BACKOFF_MS));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err
            .unwrap_or_else(|| SettingsDbError::Rusqlite(rusqlite::Error::QueryReturnedNoRows)))
    }

    /// open の 1 回ぶんの実行。pragma / integrity_check / init_schema を一気通貫で実行する。
    /// 途中のどのステップで `rusqlite::Error` が出ても、`classify_rusqlite_error_for_open`
    /// を通すことで Corrupted / Permission / Transient へ正しく分類される (Codex P1 対応)。
    fn try_open_once(path: &Path, mode: OpenMode) -> Result<Self, SettingsDbError> {
        use rusqlite::OpenFlags;
        let flags = match mode {
            OpenMode::RequireExisting => {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI
            }
            OpenMode::CreateNew => {
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_URI
            }
        };
        let conn = Connection::open_with_flags(path, flags)
            .map_err(|e| classify_rusqlite_error_for_open(e, "Connection::open"))?;
        apply_pragmas(&conn).map_err(|e| classify_rusqlite_error_for_open(e, "apply_pragmas"))?;
        check_integrity_classified(&conn)?;
        // Codex P3 v5 (2026-05-13): `RequireExisting` で開いたファイルが「形式上 OK な
        // 空 DB」であってもそのまま init_schema を走らせると defaults でロードされる事故が
        // 起きる (= bak / JSON migration への fallback を奪う)。Phase 2 で書き込まれる
        // `schema_meta.schema_version` の存在を必須条件にし、無ければ `Corrupted` 扱いで
        // 上層に bak 試行を委ねる。
        if mode == OpenMode::RequireExisting {
            ensure_existing_db_initialized(&conn)?;
        }
        // Codex P2 v7 (2026-05-13): `CreateNew` が呼ばれたが既存 DB が bootstrap 済みなら、
        // family の transient miss-detect から「clean install 経路」に誤って倒れている
        // 可能性が高い。**ここで止めないと続く `save_full(Settings::default())` で
        // ユーザー設定が上書き全消去される**。Corrupted ではなく専用の `AlreadyBootstrapped`
        // を返し、Phase 2 caller に「もう `save_full` を呼ぶな、別経路で recover しろ」と
        // 伝える。
        if mode == OpenMode::CreateNew && existing_bootstrap_marker_present(&conn)? {
            log_diag(
                "settings_db: create_new called on already-bootstrapped DB; \
                 refusing to clobber (Codex P2 v7)",
            );
            return Err(SettingsDbError::AlreadyBootstrapped);
        }
        init_schema(&conn).map_err(|e| classify_rusqlite_error_for_open(e, "init_schema"))?;

        Ok(Self {
            inner: Mutex::new(Inner {
                conn,
                last_saved_vst3_chain_hash: None,
                last_saved_vst3_slots_hash: None,
            }),
        })
    }

    /// 全テーブルを読んで `Settings` を再構築する。
    ///
    /// 完了時点で in-memory と DB が一致するので、VST3 hash を初期化する
    /// (= 起動後最初の `save_full` で無駄に DELETE+INSERT しないため)。
    pub fn load_into_settings(&self) -> Result<Settings, SettingsDbError> {
        let mut inner = self.inner.lock().map_err(|_| SettingsDbError::Poisoned)?;
        let settings = build_settings_from_db(&inner.conn)?;
        inner.last_saved_vst3_chain_hash = Some(hash_vst3_plugins(&settings.vst3_plugins));
        inner.last_saved_vst3_slots_hash = Some(hash_vst3_chain_slots(&settings.vst3_chain_slots));
        Ok(settings)
    }

    /// `Settings` を全テーブルに書き出す。
    ///
    /// rotation には触れない (= bootstrap save と user save 両方が共有)。
    /// - 小サイズ table: transaction 内で DELETE+INSERT (削除・並べ替えを反映)
    /// - VST3 大型 table: hash で変更検出、未変更なら skip
    /// - commit 成功後にのみ hash を更新する (= 「メモリ更新済み、DB 未更新」防止)
    pub fn save_full(&self, settings: &Settings) -> Result<(), SettingsDbError> {
        let mut inner = self.inner.lock().map_err(|_| SettingsDbError::Poisoned)?;

        // 1. 事前 hash 計算 (transaction 外)
        let chain_hash = hash_vst3_plugins(&settings.vst3_plugins);
        let slots_hash = hash_vst3_chain_slots(&settings.vst3_chain_slots);
        let chain_changed = inner
            .last_saved_vst3_chain_hash
            .map_or(true, |h| h != chain_hash);
        let slots_changed = inner
            .last_saved_vst3_slots_hash
            .map_or(true, |h| h != slots_hash);

        // 2. Settings 全体を Value::Object 化、複合フィールドを取り出す。
        let mut value = serde_json::to_value(settings)?;
        let map = match value.as_object_mut() {
            Some(m) => m,
            None => {
                return Err(SettingsDbError::Serde(serde::de::Error::custom(
                    "Settings did not serialize to a JSON object",
                )));
            }
        };
        let complex = extract_complex_fields(map);

        // 3. transaction 内で DB 更新
        let tx = inner.conn.unchecked_transaction()?;
        write_settings_kv(&tx, map)?;
        write_favorites(&tx, &settings.favorites)?;
        write_tags(&tx, &settings.tags)?;
        write_video_resume_positions(&tx, &settings.video_resume_positions)?;
        write_recent_apps(
            &tx,
            "recent_open_with_apps",
            &settings.recent_open_with_apps,
        )?;
        write_recent_apps(
            &tx,
            "custom_open_with_apps",
            &settings.custom_open_with_apps,
        )?;
        if chain_changed {
            write_vst3_plugins(&tx, &settings.vst3_plugins)?;
        }
        if slots_changed {
            write_vst3_chain_slots(&tx, &settings.vst3_chain_slots)?;
        }
        // Codex P1 v6 (2026-05-13): `bootstrap_complete = '1'` を schema_meta に書く。
        // この marker が **無いまま** RequireExisting で開かれた DB は「init_schema は
        // 通ったが最初の save_full 前に crash した状態」と判別され、Corrupted 扱いで
        // bak / JSON fallback に倒れる (= defaults 上書き事故の追加防御)。
        // INSERT OR IGNORE で冪等 (= 2 回目以降は no-op)。schema_meta 自体は init_schema
        // で作成済み。本 marker は **commit と同 transaction 内** で書く必要がある
        // (= "save_full の中身が永続化された瞬間に marker も付く" 不変条件)。
        tx.execute(
            "INSERT OR IGNORE INTO schema_meta(key, value) VALUES ('bootstrap_complete', '1')",
            [],
        )?;
        // 補足: complex は読み捨ててもよいが、unused 警告を避けるために保持する
        // (= 将来 complex 側だけ partial save する API を追加するときに使う)。
        let _ = complex;
        tx.commit()?;

        // 4. commit 成功してから hash 更新
        if chain_changed {
            inner.last_saved_vst3_chain_hash = Some(chain_hash);
        }
        if slots_changed {
            inner.last_saved_vst3_slots_hash = Some(slots_hash);
        }
        Ok(())
    }

    /// `VACUUM INTO` で snapshot を作成する。
    ///
    /// `target` が存在しないことを呼び出し側で保証する (SQLite の VACUUM INTO は
    /// target が既存だとエラーになる)。世代 rotation の Phase 5 で使う。
    pub fn backup_to(&self, target: &Path) -> Result<(), SettingsDbError> {
        let inner = self.inner.lock().map_err(|_| SettingsDbError::Poisoned)?;
        // VACUUM INTO はパラメータ binding が使えないので、target をエスケープして
        // 埋め込む。target は呼出側 (rotate_db_backups) が生成する制御パスなので
        // injection リスクは低いが、シングルクオートだけ二重化する。
        let target_str = target.to_string_lossy();
        let escaped = target_str.replace('\'', "''");
        let sql = format!("VACUUM INTO '{escaped}'");
        inner.conn.execute_batch(&sql)?;
        Ok(())
    }

    /// `schema_meta.migrated_from_json_at = <unix_ts>` を冪等に書く。
    /// Phase 2 で JSON migration を完了したときに 1 回だけ呼ぶ。既存の row があれば
    /// 上書きせずそのまま (= 最初の migration 時刻が安定する)。
    pub fn record_migrated_from_json(&self) -> Result<(), SettingsDbError> {
        let inner = self.inner.lock().map_err(|_| SettingsDbError::Poisoned)?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        inner.conn.execute(
            "INSERT OR IGNORE INTO schema_meta(key, value) VALUES ('migrated_from_json_at', ?1)",
            params![ts.to_string()],
        )?;
        Ok(())
    }

    /// テスト用: in-memory SQLite を直接開く。
    #[cfg(test)]
    fn open_in_memory_for_test() -> Result<Self, SettingsDbError> {
        let conn = Connection::open_in_memory()?;
        apply_pragmas(&conn)?;
        init_schema(&conn)?;
        Ok(Self {
            inner: Mutex::new(Inner {
                conn,
                last_saved_vst3_chain_hash: None,
                last_saved_vst3_slots_hash: None,
            }),
        })
    }
}

// ---------------------------------------------------------------------------
// パス / family 判定
// ---------------------------------------------------------------------------

pub fn settings_db_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.db")
}

/// `settings.db` family (本体 + WAL/SHM + bak1..bak10) のいずれかが物理的に
/// 存在するか。
///
/// 単一 `metadata()` だと transient NotFound で誤判定するので per-file metadata と
/// `read_dir` 両経路で確認する (spec §5)。1 つでも見えれば true。
pub fn settings_db_family_exists(data_dir: &Path) -> bool {
    // 経路 1: per-file metadata
    let candidates = family_filenames();
    for name in &candidates {
        let p = data_dir.join(name);
        if std::fs::metadata(&p).is_ok() {
            return true;
        }
    }
    // 経路 2: read_dir で列挙
    if let Ok(entries) = std::fs::read_dir(data_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if is_family_filename(name) {
                    return true;
                }
            }
        }
    }
    false
}

fn family_filenames() -> Vec<String> {
    let mut v = vec![
        "settings.db".to_string(),
        "settings.db-wal".to_string(),
        "settings.db-shm".to_string(),
    ];
    for i in 1..=10 {
        v.push(format!("settings.db.bak{i}"));
    }
    v
}

fn is_family_filename(name: &str) -> bool {
    if name == "settings.db" || name == "settings.db-wal" || name == "settings.db-shm" {
        return true;
    }
    if let Some(rest) = name.strip_prefix("settings.db.bak") {
        // spec §6 で定義された世代 = **正準形の** bak1..bak10 のみ。
        // 範囲外 (bak0 / bak11 / bak100) と先頭ゼロ (bak01) は family 扱いしない
        // (Codex P3 v4/v5 2026-05-13: 異物を family と誤判定すると、main 不在時に
        // save 抑止に倒れてクリーンインストール経路が永久に動かなくなる)。
        // 「rotate_db_backups が生成する canonical 名以外は無視する」ポリシー。
        if let Ok(n) = rest.parse::<u32>() {
            // 先頭ゼロを弾く: parse は "01" -> 1 を許すが正準形 1 桁とは違う。
            if rest == n.to_string() {
                return (1..=10).contains(&n);
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// open / pragmas / schema
// ---------------------------------------------------------------------------

fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    // journal_mode=WAL は in-memory DB で no-op になるので silently ignore する
    // (rusqlite が "memory" を返すケース)。本番では WAL に切り替わる。
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA wal_autocheckpoint = 1000;
         PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;",
    )?;
    Ok(())
}

/// bootstrap 済み DB に存在しなければならないテーブル一覧 (Codex P2 v11 2026-05-14)。
/// 1 つでも消えていたら `init_schema` の `CREATE IF NOT EXISTS` で silently 再作成
/// される → empty で load → save_full で完全消去、を防ぐために事前検査する。
const REQUIRED_TABLES_AFTER_BOOTSTRAP: &[&str] = &[
    "schema_meta",
    "settings_kv",
    "favorites",
    "tags",
    "video_resume_positions",
    "vst3_plugins",
    "vst3_chain_slots",
    "recent_open_with_apps",
    "custom_open_with_apps",
];

/// `RequireExisting` モードで開いた DB が **bootstrap 完了済み** (= 最初の save_full が
/// 成功して中身が書かれた) 既存ファイルであることを保証する。
///
/// SQLite は空ファイル (0 バイト) や形式上有効な空 DB を `Connection::open` で
/// 受け付けてしまう。`init_schema` だけが走り `save_full` 前に crash したケースも
/// 「テーブルとスキーマだけ存在し中身ゼロ」になる。そのまま defaults でロードすると
/// bak / JSON migration への fallback 経路が奪われる
/// (Codex P3 v5 + P1 v6 2026-05-13)。
///
/// 判定段階:
/// 1. `schema_meta` テーブルが存在するか (= 完全に空の DB 検出)
/// 2. `schema_version` row があるか (= init_schema 終了確認、partial-init 検出)
/// 3. `bootstrap_complete = '1'` row があるか (= 最初の save_full がコミットされたか)
/// 4. 必須テーブル全部が存在するか (Codex P2 v11 2026-05-14: テーブル単位の消失検出)
///
/// 3 の marker は `save_full` の transaction 内で書く (= 中身と同じ瞬間に永続化される)。
/// この四段確認で「init_schema 後 / save_full 前に crash」「個別テーブル消失」のいずれも
/// Corrupted として bak 経路へ倒す。
fn ensure_existing_db_initialized(conn: &Connection) -> Result<(), SettingsDbError> {
    // schema_meta テーブル自体の存在をまず見る。
    let table_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_meta'",
            [],
            |_| Ok(true),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            _ => Err(e),
        })
        .map_err(|e| classify_rusqlite_error_for_open(e, "ensure_existing_db_initialized:table"))?;
    if !table_exists {
        log_diag(
            "settings_db: RequireExisting open: schema_meta table missing (empty or partial DB)",
        );
        return Err(SettingsDbError::Corrupted(
            "RequireExisting: schema_meta table missing".to_string(),
        ));
    }
    // schema_version row が無ければ partial-init として Corrupted 扱い。
    let has_version: bool = conn
        .query_row(
            "SELECT 1 FROM schema_meta WHERE key = 'schema_version'",
            [],
            |_| Ok(true),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            _ => Err(e),
        })
        .map_err(|e| {
            classify_rusqlite_error_for_open(e, "ensure_existing_db_initialized:version")
        })?;
    if !has_version {
        log_diag("settings_db: RequireExisting open: schema_version row missing");
        return Err(SettingsDbError::Corrupted(
            "RequireExisting: schema_version row missing".to_string(),
        ));
    }
    // bootstrap_complete marker (Codex P1 v6): save_full が **一度も commit を成功させて
    // いない** 状態を検出する。`init_schema` だけ通って save_full 前に crash したケースは
    // schema_version はあるがこの marker は無い。
    let has_bootstrap: bool = conn
        .query_row(
            "SELECT 1 FROM schema_meta WHERE key = 'bootstrap_complete'",
            [],
            |_| Ok(true),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            _ => Err(e),
        })
        .map_err(|e| {
            classify_rusqlite_error_for_open(e, "ensure_existing_db_initialized:bootstrap")
        })?;
    if !has_bootstrap {
        log_diag(
            "settings_db: RequireExisting open: bootstrap_complete marker missing \
             (init_schema ran but no save_full committed)",
        );
        return Err(SettingsDbError::Corrupted(
            "RequireExisting: bootstrap_complete marker missing".to_string(),
        ));
    }
    // Codex P2 v11 (2026-05-14): 個別テーブルの消失検出。`init_schema` の
    // CREATE IF NOT EXISTS が silently 再作成して empty で load される事故を防ぐ。
    let mut stmt = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1")
        .map_err(|e| classify_rusqlite_error_for_open(e, "ensure_existing_db_initialized:enum"))?;
    for table in REQUIRED_TABLES_AFTER_BOOTSTRAP {
        let exists: bool = stmt
            .query_row([table], |_| Ok(true))
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(false),
                _ => Err(e),
            })
            .map_err(|e| {
                classify_rusqlite_error_for_open(e, "ensure_existing_db_initialized:table_check")
            })?;
        if !exists {
            log_diag(&format!(
                "settings_db: RequireExisting open: required table '{table}' missing"
            ));
            return Err(SettingsDbError::Corrupted(format!(
                "RequireExisting: required table '{table}' missing"
            )));
        }
    }
    Ok(())
}

/// `schema_meta.bootstrap_complete` row が既に存在するか確認する
/// (Codex P2 v7 2026-05-13)。`schema_meta` テーブル自体が無い場合 (= 完全に空の DB) は
/// false を返す (= bootstrap 未完了)。
fn existing_bootstrap_marker_present(conn: &Connection) -> Result<bool, SettingsDbError> {
    // schema_meta が無いと query が落ちるので、まずテーブル存在チェック。
    let table_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_meta'",
            [],
            |_| Ok(true),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            _ => Err(e),
        })
        .map_err(|e| classify_rusqlite_error_for_open(e, "bootstrap_marker:table"))?;
    if !table_exists {
        return Ok(false);
    }
    let present: bool = conn
        .query_row(
            "SELECT 1 FROM schema_meta WHERE key = 'bootstrap_complete'",
            [],
            |_| Ok(true),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            _ => Err(e),
        })
        .map_err(|e| classify_rusqlite_error_for_open(e, "bootstrap_marker:row"))?;
    Ok(present)
}

/// `PRAGMA integrity_check(1)` を回し、SQLite エラーは open 系として分類、
/// 戻り文字列が "ok" 以外なら `Corrupted` を返す (Codex P1 対応で旧
/// `check_integrity` を retire したものの代替)。
fn check_integrity_classified(conn: &Connection) -> Result<(), SettingsDbError> {
    let result: String = conn
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(|e| classify_rusqlite_error_for_open(e, "integrity_check"))?;
    if result.eq_ignore_ascii_case("ok") {
        Ok(())
    } else {
        log_diag(&format!(
            "settings_db: integrity_check returned non-ok: {result}"
        ));
        Err(SettingsDbError::Corrupted(format!(
            "integrity_check returned: {result}"
        )))
    }
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
         );

         CREATE TABLE IF NOT EXISTS settings_kv (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
         );

         CREATE TABLE IF NOT EXISTS favorites (
            id                    BLOB PRIMARY KEY,
            name                  TEXT NOT NULL,
            path                  TEXT NOT NULL,
            sort_index            INTEGER NOT NULL,
            auto_index_structure  INTEGER NOT NULL DEFAULT 0,
            auto_index_metadata   INTEGER NOT NULL DEFAULT 0,
            auto_index_thumbs     INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS favorites_sort ON favorites(sort_index);

         CREATE TABLE IF NOT EXISTS tags (
            id          BLOB PRIMARY KEY,
            name        TEXT NOT NULL,
            sort_index  INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS tags_sort ON tags(sort_index);

         CREATE TABLE IF NOT EXISTS video_resume_positions (
            path_normalized TEXT PRIMARY KEY,
            position_secs   REAL NOT NULL,
            updated_at      INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS vst3_plugins (
            plugin_path  TEXT PRIMARY KEY,
            chain_index  INTEGER NOT NULL UNIQUE,
            plugin_name  TEXT,
            bypass       INTEGER NOT NULL DEFAULT 0,
            user_hidden  INTEGER NOT NULL DEFAULT 0,
            gui_pos_x    INTEGER,
            gui_pos_y    INTEGER,
            gui_size_w   INTEGER,
            gui_size_h   INTEGER,
            state        TEXT
         );

         CREATE TABLE IF NOT EXISTS vst3_chain_slots (
            slot_index     INTEGER PRIMARY KEY,
            name           TEXT NOT NULL,
            gui_visible    INTEGER NOT NULL DEFAULT 1,
            video_compact  INTEGER NOT NULL DEFAULT 0,
            plugins_json   TEXT NOT NULL
         );

         CREATE TABLE IF NOT EXISTS recent_open_with_apps (
            exe_path      TEXT PRIMARY KEY,
            display_name  TEXT NOT NULL,
            sort_index    INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS recent_apps_sort
            ON recent_open_with_apps(sort_index);

         CREATE TABLE IF NOT EXISTS custom_open_with_apps (
            exe_path      TEXT PRIMARY KEY,
            display_name  TEXT NOT NULL,
            sort_index    INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS custom_apps_sort
            ON custom_open_with_apps(sort_index);",
    )?;

    // schema_meta: schema_version / app_version を冪等に upsert する。
    // migrated_from_json_at は Phase 2 (JSON migration) でセットされる。
    let app_version = env!("CARGO_PKG_VERSION");
    conn.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SCHEMA_VERSION],
    )?;
    conn.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('app_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![app_version],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// settings_kv (scalar / 小 array / 中 struct)
// ---------------------------------------------------------------------------

fn extract_complex_fields(map: &mut Map<String, Value>) -> Map<String, Value> {
    let mut taken = Map::new();
    for name in COMPLEX_FIELDS {
        if let Some(v) = map.remove(*name) {
            taken.insert((*name).to_string(), v);
        }
    }
    taken
}

fn write_settings_kv(
    tx: &rusqlite::Transaction<'_>,
    map: &Map<String, Value>,
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM settings_kv", [])?;
    let mut stmt = tx.prepare("INSERT INTO settings_kv(key, value) VALUES (?1, ?2)")?;
    for (k, v) in map.iter() {
        // serde_json::to_string は基本的に失敗しないが、念のため to_string を経由する。
        let s = v.to_string();
        stmt.execute(params![k, s])?;
    }
    Ok(())
}

fn read_settings_kv(conn: &Connection) -> Result<Map<String, Value>, SettingsDbError> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings_kv")?;
    let rows = stmt.query_map([], |row| {
        let k: String = row.get(0)?;
        let v: String = row.get(1)?;
        Ok((k, v))
    })?;
    let mut map = Map::new();
    for row in rows {
        let (k, raw) = row?;
        // Codex P2 v10 (2026-05-14): settings_kv 行が壊れた JSON だったら Corrupted で返す
        // (favorites.id / plugins_json と対称)。Serde で返すと Phase 2 の bak fallback 経路に
        // 拾われない懸念があるため。
        let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
            SettingsDbError::Corrupted(format!("settings_kv[{k}] value not valid JSON: {e}"))
        })?;
        map.insert(k, parsed);
    }
    Ok(map)
}

// ---------------------------------------------------------------------------
// favorites
// ---------------------------------------------------------------------------

fn write_favorites(
    tx: &rusqlite::Transaction<'_>,
    favorites: &[FavoriteEntry],
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM favorites", [])?;
    let mut stmt = tx.prepare(
        "INSERT INTO favorites
            (id, name, path, sort_index,
             auto_index_structure, auto_index_metadata, auto_index_thumbs)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (idx, fav) in favorites.iter().enumerate() {
        stmt.execute(params![
            fav.id.as_bytes().as_slice(),
            fav.name,
            fav.path.to_string_lossy().as_ref(),
            idx as i64,
            fav.auto_index_structure as i64,
            fav.auto_index_metadata as i64,
            fav.auto_index_thumbs as i64,
        ])?;
    }
    Ok(())
}

fn read_favorites(conn: &Connection) -> Result<Vec<FavoriteEntry>, SettingsDbError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, path, auto_index_structure, auto_index_metadata, auto_index_thumbs
         FROM favorites
         ORDER BY sort_index ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let id_bytes: Vec<u8> = row.get(0)?;
        let name: String = row.get(1)?;
        let path: String = row.get(2)?;
        let auto_index_structure: i64 = row.get(3)?;
        let auto_index_metadata: i64 = row.get(4)?;
        let auto_index_thumbs: i64 = row.get(5)?;
        Ok((
            id_bytes,
            name,
            path,
            auto_index_structure != 0,
            auto_index_metadata != 0,
            auto_index_thumbs != 0,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id_bytes, name, path, ais, aim, ait) = row?;
        // Codex P2 v8 (2026-05-13): UUID 長が 16 でないなら row 破損。silently nil に
        // fallback すると save_full 後に「ユーザーが個別 favorite を編集していた状態」
        // が消える。Corrupted で上層に伝え、bak / JSON migration へ倒す。
        let id = Uuid::from_slice(&id_bytes).map_err(|e| {
            SettingsDbError::Corrupted(format!(
                "favorites.id bytes invalid (len={}, name={name:?}): {e}",
                id_bytes.len()
            ))
        })?;
        out.push(FavoriteEntry {
            id,
            name,
            path: PathBuf::from(path),
            auto_index_structure: ais,
            auto_index_metadata: aim,
            auto_index_thumbs: ait,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// tags
// ---------------------------------------------------------------------------

fn write_tags(tx: &rusqlite::Transaction<'_>, tags: &[TagDef]) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM tags", [])?;
    let mut stmt = tx.prepare("INSERT INTO tags (id, name, sort_index) VALUES (?1, ?2, ?3)")?;
    for (idx, tag) in tags.iter().enumerate() {
        stmt.execute(params![tag.id.as_bytes().as_slice(), tag.name, idx as i64,])?;
    }
    Ok(())
}

fn read_tags(conn: &Connection) -> Result<Vec<TagDef>, SettingsDbError> {
    let mut stmt = conn.prepare("SELECT id, name FROM tags ORDER BY sort_index ASC")?;
    let rows = stmt.query_map([], |row| {
        let id_bytes: Vec<u8> = row.get(0)?;
        let name: String = row.get(1)?;
        Ok((id_bytes, name))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id_bytes, name) = row?;
        // Codex P2 v8: 同上。silently new_v4 すると tag id が安定でなくなる。
        let id = Uuid::from_slice(&id_bytes).map_err(|e| {
            SettingsDbError::Corrupted(format!(
                "tags.id bytes invalid (len={}, name={name:?}): {e}",
                id_bytes.len()
            ))
        })?;
        out.push(TagDef { id, name });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// video_resume_positions
// ---------------------------------------------------------------------------

fn write_video_resume_positions(
    tx: &rusqlite::Transaction<'_>,
    map: &std::collections::HashMap<String, f64>,
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM video_resume_positions", [])?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut stmt = tx.prepare(
        "INSERT INTO video_resume_positions (path_normalized, position_secs, updated_at)
         VALUES (?1, ?2, ?3)",
    )?;
    for (path, secs) in map.iter() {
        // f64 が非有限値 (NaN / Inf) で入ってきた場合は弾く (SQLite の REAL は IEEE 754
        // を許容するが、後段で round/clamp する箇所で問題化するため)。
        if !secs.is_finite() {
            continue;
        }
        stmt.execute(params![path, secs, now])?;
    }
    Ok(())
}

fn read_video_resume_positions(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, f64>, SettingsDbError> {
    let mut stmt =
        conn.prepare("SELECT path_normalized, position_secs FROM video_resume_positions")?;
    let rows = stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let secs: f64 = row.get(1)?;
        Ok((path, secs))
    })?;
    let mut out = std::collections::HashMap::new();
    for row in rows {
        let (path, secs) = row?;
        if secs.is_finite() {
            out.insert(path, secs);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// vst3_plugins
// ---------------------------------------------------------------------------

fn write_vst3_plugins(
    tx: &rusqlite::Transaction<'_>,
    plugins: &[Vst3PluginEntry],
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM vst3_plugins", [])?;
    let mut stmt = tx.prepare(
        "INSERT INTO vst3_plugins
            (plugin_path, chain_index, plugin_name,
             bypass, user_hidden,
             gui_pos_x, gui_pos_y, gui_size_w, gui_size_h, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for (idx, p) in plugins.iter().enumerate() {
        let plugin_name: Option<String> = None; // 現状 Vst3PluginEntry に name 無し
        let (gpx, gpy) = p
            .gui_pos
            .map(|(x, y)| (Some(x as i64), Some(y as i64)))
            .unwrap_or((None, None));
        let (gsw, gsh) = p
            .gui_size
            .map(|(w, h)| (Some(w as i64), Some(h as i64)))
            .unwrap_or((None, None));
        stmt.execute(params![
            p.path,
            idx as i64,
            plugin_name,
            p.bypass as i64,
            p.user_hidden as i64,
            gpx,
            gpy,
            gsw,
            gsh,
            p.state,
        ])?;
    }
    Ok(())
}

fn read_vst3_plugins(conn: &Connection) -> Result<Vec<Vst3PluginEntry>, SettingsDbError> {
    let mut stmt = conn.prepare(
        "SELECT plugin_path, bypass, user_hidden,
                gui_pos_x, gui_pos_y, gui_size_w, gui_size_h, state
         FROM vst3_plugins
         ORDER BY chain_index ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let bypass: i64 = row.get(1)?;
        let user_hidden: i64 = row.get(2)?;
        let gpx: Option<i64> = row.get(3)?;
        let gpy: Option<i64> = row.get(4)?;
        let gsw: Option<i64> = row.get(5)?;
        let gsh: Option<i64> = row.get(6)?;
        let state: Option<String> = row.get(7)?;
        Ok((path, bypass, user_hidden, gpx, gpy, gsw, gsh, state))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (path, bypass, user_hidden, gpx, gpy, gsw, gsh, state) = row?;
        let gui_pos = match (gpx, gpy) {
            (Some(x), Some(y)) => Some((x as i32, y as i32)),
            _ => None,
        };
        let gui_size = match (gsw, gsh) {
            (Some(w), Some(h)) if w >= 0 && h >= 0 => Some((w as u32, h as u32)),
            _ => None,
        };
        out.push(Vst3PluginEntry {
            path,
            bypass: bypass != 0,
            state,
            user_hidden: user_hidden != 0,
            gui_pos,
            gui_size,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// vst3_chain_slots
// ---------------------------------------------------------------------------

fn write_vst3_chain_slots(
    tx: &rusqlite::Transaction<'_>,
    slots: &Vst3ChainPresetSlots,
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM vst3_chain_slots", [])?;
    let mut stmt = tx.prepare(
        "INSERT INTO vst3_chain_slots
            (slot_index, name, gui_visible, video_compact, plugins_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (idx, slot_opt) in slots.slots.iter().enumerate() {
        if let Some(slot) = slot_opt {
            let plugins_json = match serde_json::to_string(&slot.plugins) {
                Ok(s) => s,
                // ここに到達するなら serde 側の bug。空配列にフォールバック。
                Err(_) => "[]".to_string(),
            };
            stmt.execute(params![
                idx as i64,
                slot.name,
                slot.gui_visible as i64,
                slot.video_compact as i64,
                plugins_json,
            ])?;
        }
    }
    Ok(())
}

fn read_vst3_chain_slots(conn: &Connection) -> Result<Vst3ChainPresetSlots, SettingsDbError> {
    let mut stmt = conn.prepare(
        "SELECT slot_index, name, gui_visible, video_compact, plugins_json
         FROM vst3_chain_slots
         ORDER BY slot_index ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let idx: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let gui_visible: i64 = row.get(2)?;
        let video_compact: i64 = row.get(3)?;
        let plugins_json: String = row.get(4)?;
        Ok((idx, name, gui_visible, video_compact, plugins_json))
    })?;
    let mut out = Vst3ChainPresetSlots::default();
    let max_slot = out.slots.len();
    for row in rows {
        let (idx, name, gui_visible, video_compact, plugins_json) = row?;
        // Codex P2 v9 (2026-05-14): out-of-range slot_index は silently skip せず Corrupted。
        // skip すると次の save_full DELETE+INSERT で当該 preset 行が永久に消えるため、
        // plugins_json の壊れ方と同じく上層の bak / JSON fallback に倒す。
        if idx < 0 || (idx as usize) >= max_slot {
            return Err(SettingsDbError::Corrupted(format!(
                "vst3_chain_slots.slot_index out of range: {idx} (max {})",
                max_slot - 1
            )));
        }
        // Codex P2 v8 (2026-05-13): plugins_json が壊れていれば slot 全体を捨てるのではなく
        // Corrupted を返して上層の bak / JSON fallback に倒す (= silently empty slot に
        // ならないようにする)。
        let plugins: Vec<Vst3PluginEntry> = serde_json::from_str(&plugins_json).map_err(|e| {
            SettingsDbError::Corrupted(format!("vst3_chain_slots[{idx}].plugins_json invalid: {e}"))
        })?;
        out.slots[idx as usize] = Some(Vst3ChainPresetSlot {
            name,
            plugins,
            gui_visible: gui_visible != 0,
            video_compact: video_compact != 0,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// recent / custom open-with apps
// ---------------------------------------------------------------------------

fn write_recent_apps(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    apps: &[RecentApp],
) -> rusqlite::Result<()> {
    // テーブル名はコード内固定の 2 値のみ (`recent_open_with_apps`,
    // `custom_open_with_apps`)。injection リスクなし。
    tx.execute(&format!("DELETE FROM {table}"), [])?;
    let mut stmt = tx.prepare(&format!(
        "INSERT INTO {table} (exe_path, display_name, sort_index) VALUES (?1, ?2, ?3)"
    ))?;
    for (idx, app) in apps.iter().enumerate() {
        stmt.execute(params![app.exe_path, app.display_name, idx as i64])?;
    }
    Ok(())
}

fn read_recent_apps(conn: &Connection, table: &str) -> Result<Vec<RecentApp>, SettingsDbError> {
    let sql = format!("SELECT exe_path, display_name FROM {table} ORDER BY sort_index ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let exe_path: String = row.get(0)?;
        let display_name: String = row.get(1)?;
        Ok(RecentApp {
            exe_path,
            display_name,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// load 側: 全テーブル → Settings
// ---------------------------------------------------------------------------

fn build_settings_from_db(conn: &Connection) -> Result<Settings, SettingsDbError> {
    let mut map = read_settings_kv(conn)?;

    // 複合テーブルを読み、JSON Value にして Map に挿入する。
    let favorites = read_favorites(conn)?;
    let tags = read_tags(conn)?;
    let video_resume_positions = read_video_resume_positions(conn)?;
    let vst3_plugins = read_vst3_plugins(conn)?;
    let vst3_chain_slots = read_vst3_chain_slots(conn)?;
    let recent_apps = read_recent_apps(conn, "recent_open_with_apps")?;
    let custom_apps = read_recent_apps(conn, "custom_open_with_apps")?;

    map.insert("favorites".into(), serde_json::to_value(favorites)?);
    map.insert("tags".into(), serde_json::to_value(tags)?);
    map.insert(
        "video_resume_positions".into(),
        serde_json::to_value(video_resume_positions)?,
    );
    map.insert("vst3_plugins".into(), serde_json::to_value(vst3_plugins)?);
    map.insert(
        "vst3_chain_slots".into(),
        serde_json::to_value(vst3_chain_slots)?,
    );
    map.insert(
        "recent_open_with_apps".into(),
        serde_json::to_value(recent_apps)?,
    );
    map.insert(
        "custom_open_with_apps".into(),
        serde_json::to_value(custom_apps)?,
    );

    // Codex P2 v10 (2026-05-14): settings_kv の scalar 形が不正 (例: grid_cols が String) で
    // from_value が失敗するケースは DB 内容の corruption。Serde で返すと Phase 2 の bak
    // fallback 経路に拾われない可能性があるため Corrupted に統一する。
    let settings: Settings = serde_json::from_value(Value::Object(map)).map_err(|e| {
        SettingsDbError::Corrupted(format!("settings_kv shape mismatch in from_value: {e}"))
    })?;
    Ok(settings)
}

// ---------------------------------------------------------------------------
// hash (VST3 dirty 検出用)
// ---------------------------------------------------------------------------

fn hash_vst3_plugins(plugins: &[Vst3PluginEntry]) -> u64 {
    let mut h = DefaultHasher::new();
    plugins.len().hash(&mut h);
    for p in plugins {
        p.path.hash(&mut h);
        p.bypass.hash(&mut h);
        p.user_hidden.hash(&mut h);
        p.state.hash(&mut h);
        p.gui_pos.hash(&mut h);
        p.gui_size.hash(&mut h);
    }
    h.finish()
}

fn hash_vst3_chain_slots(slots: &Vst3ChainPresetSlots) -> u64 {
    let mut h = DefaultHasher::new();
    slots.slots.len().hash(&mut h);
    for slot_opt in slots.slots.iter() {
        match slot_opt {
            None => 0u8.hash(&mut h),
            Some(s) => {
                1u8.hash(&mut h);
                s.name.hash(&mut h);
                s.gui_visible.hash(&mut h);
                s.video_compact.hash(&mut h);
                s.plugins.len().hash(&mut h);
                for p in s.plugins.iter() {
                    p.path.hash(&mut h);
                    p.bypass.hash(&mut h);
                    p.user_hidden.hash(&mut h);
                    p.state.hash(&mut h);
                    p.gui_pos.hash(&mut h);
                    p.gui_size.hash(&mut h);
                }
            }
        }
    }
    h.finish()
}

// ---------------------------------------------------------------------------
// Phase 2: quarantine / JSON migration / boot decision tree (spec §5)
// ---------------------------------------------------------------------------

/// quarantine 名のユニーク化 counter (Codex P2 v8b-4 2026-05-14)。
/// 同一秒内に複数回 quarantine を呼ぶケース (= bak から復旧 → 再失敗 → quarantine)
/// で `.corrupted-<ts>` が衝突するのを防ぐ。
static QUARANTINE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `<data_dir>/settings.db{,-wal,-shm}` を 3 セットで `.corrupted-<ts>-<seq>` にリネームする。
///
/// spec §6.2 に従い、Corrupted 検出時に main DB だけでなく WAL / SHM も一緒に退避する。
/// これで新しい DB を作り直したときに古い WAL の recovery で誤った内容が混入するのを防ぐ。
/// ファイルが無いものは無視する (= NotFound はエラーにしない)。
///
/// 同一秒内の複数回呼び出し (= bak から復旧 → 再失敗 → 再 quarantine) で `.corrupted-<ts>`
/// が衝突して rename が失敗するのを防ぐため、unix epoch 秒 + プロセス内 atomic counter で
/// suffix を一意化する (Codex P2 v8b-4 2026-05-14)。
///
/// 戻り値: 退避したファイル数 (= 0 でも error にはしない、main が transient missing の
/// ケースも含むため)。
pub fn quarantine_db_files(data_dir: &Path) -> usize {
    use std::sync::atomic::Ordering;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let seq = QUARANTINE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let names = [
        "settings.db".to_string(),
        "settings.db-wal".to_string(),
        "settings.db-shm".to_string(),
    ];
    let mut moved = 0usize;
    for name in &names {
        let src = data_dir.join(name);
        if !src.exists() {
            continue;
        }
        let dst = data_dir.join(format!("{name}.corrupted-{ts}-{seq}"));
        match std::fs::rename(&src, &dst) {
            Ok(_) => {
                log_diag(&format!(
                    "settings_db: quarantined {} -> {}",
                    src.display(),
                    dst.display()
                ));
                moved += 1;
            }
            Err(e) => {
                log_diag(&format!(
                    "settings_db: quarantine failed {} -> {}: {e}",
                    src.display(),
                    dst.display()
                ));
            }
        }
    }
    moved
}

/// `<data_dir>` 配下の旧 `settings.json{,.bak1..bak10}` を `.migrated-<ts>` にリネームする。
/// `data_dir` 引数を**唯一の真**として使う (= グローバル `data_dir::get()` には依存しない、
/// Codex P2 v8b-2 2026-05-14)。これで data_dir override と引数が乖離しても DB と JSON が
/// 別 dir に分裂しない。
///
/// 戻り値: リネームしたファイル数。リネーム失敗は個別に log するが abort はしない
/// (= 一部成功 + 一部失敗が起きても migration 全体は通す)。
fn rename_legacy_json_files(data_dir: &Path) -> usize {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let main = data_dir.join("settings.json");
    let files = crate::settings::legacy_json_files_for_migration(&main);
    let mut moved = 0;
    for src in files {
        let mut name = match src.file_name() {
            Some(n) => n.to_owned(),
            None => continue,
        };
        name.push(format!(".migrated-{ts}"));
        let dst = src.with_file_name(name);
        match std::fs::rename(&src, &dst) {
            Ok(_) => {
                log_diag(&format!(
                    "settings_db: migrated json renamed {} -> {}",
                    src.display(),
                    dst.display()
                ));
                moved += 1;
            }
            Err(e) => {
                log_diag(&format!(
                    "settings_db: migrate rename failed {} -> {}: {e}",
                    src.display(),
                    dst.display()
                ));
            }
        }
    }
    moved
}

/// 旧 `settings.json` (+ bak1..bak10) を読み取り、`<data_dir>/settings.db` を作成して
/// 内容を保存し、旧 JSON ファイルを `.migrated-<ts>` にリネームする (spec §5 + §7)。
///
/// 戻り値:
/// - `Ok((db, settings))` — migration 成功。Phase 3 caller は `settings` を使う。
/// - `Err(SettingsDbError)` — JSON が読めない / DB create に失敗 / save に失敗。
///
/// 注意: 副作用順序は spec §7 に従い「DB 作成 → save_full → JSON リネーム」。途中で
/// crash しても旧 JSON が残っているので次回起動でやり直せる (= 二重 migration は
/// `family が見えるので migrate しない` 分岐で自然に排除される)。
pub fn migrate_from_settings_json(
    data_dir: &Path,
) -> Result<(SettingsDb, Settings), SettingsDbError> {
    // Codex P2 v8b-2 (2026-05-14): data_dir 引数を **唯一の真** として使う。
    // `data_dir::get()` 経由のパスは一切経由しない。これで data_dir override と
    // 引数が乖離しても DB と JSON が別 dir に分裂しない。
    let main = data_dir.join("settings.json");
    let mut loaded = crate::settings::read_settings_json_for_migration(&main).ok_or_else(|| {
        log_diag("settings_db: migrate_from_settings_json: no readable JSON found");
        SettingsDbError::Corrupted("no readable settings.json (or bak) for migration".to_string())
    })?;

    // Codex P2 v8b-1 (2026-05-14): Settings::load 経路と同じ load-time migrations を
    // 適用してから DB に書く。これをしないと:
    // - favorites の id が全部 Uuid::nil() で残り PRIMARY KEY 衝突 → save_full 失敗
    // - 旧 vst3_plugin_path/state が新 Vec に流れない
    // - 旧 video_loop=true が video_loop_mode に伝わらない
    crate::settings::apply_load_time_migrations(&mut loaded);

    // 新規 DB を bootstrap。family が見える環境では `create_new` が AlreadyBootstrapped を
    // 返すので、Phase 2 caller が migrate_from_settings_json を呼ぶ前に
    // `settings_db_family_exists()` で false を確認している前提。
    let db = SettingsDb::create_new(data_dir)?;
    db.save_full(&loaded)?;
    db.record_migrated_from_json()?;
    let moved = rename_legacy_json_files(data_dir);
    log_diag(&format!(
        "settings_db: migrate_from_settings_json: bootstrap complete, {moved} legacy file(s) renamed"
    ));
    Ok((db, loaded))
}

/// spec §5 の起動時決定木の入口。Phase 3 で `Settings::load` から呼ぶ想定。
///
/// 戻り値の `BootOutcome` を見て:
/// - `settings` を in-memory state として使う
/// - `db.is_some()` ならその DB が今セッションの永続化先。`save_full` で write する。
/// - `db.is_none()` (= `suppress_save == true`) なら本セッションは save 抑止する
///   (現行 `MAIN_UNREADABLE_THIS_SESSION` 相当)。
///
/// 副作用: 成功時に `GLOBAL_DB` を初期化する (= 後続の `with_db` がブロックせず動く)。
/// 失敗時は `GLOBAL_DB` をクリアする (= save 抑止状態)。
pub fn boot_settings_db(data_dir: &Path) -> BootOutcome {
    let outcome = boot_settings_db_inner(data_dir);
    // GLOBAL_DB の同期。Phase 3 で `Settings::save()` が `with_db` 経由で書く。
    set_global_db(data_dir, outcome.db.clone());
    outcome
}

pub struct BootOutcome {
    /// このセッションで使う Settings。
    pub settings: Settings,
    /// 永続化先 DB ハンドル。`None` なら本セッションは save を一切しない (= 残骸保護)。
    pub db: Option<Arc<SettingsDb>>,
    /// 起動経路 (telemetry / log 用)。
    pub source: BootSource,
}

impl std::fmt::Debug for BootOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootOutcome")
            .field("settings", &"<Settings>")
            .field("db", &self.db.as_ref().map(|_| "<SettingsDb>"))
            .field("source", &self.source)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootSource {
    /// 既存の `settings.db` を開いて load した。
    LoadedExistingDb,
    /// 既存 main DB が壊れていたので bak1..bak10 から復旧した。
    RestoredFromDbBackup,
    /// `settings.json` から migration して新規 `settings.db` を作った。
    MigratedFromJson,
    /// `settings.json` も `settings.db` も無く、clean install として bootstrap した。
    CleanInstall,
    /// すべての復旧経路が失敗。Defaults を返し、本セッションでは save を抑止する。
    FailedFallbackDefault,
}

fn boot_settings_db_inner(data_dir: &Path) -> BootOutcome {
    // 1. family が見えるか
    if settings_db_family_exists(data_dir) {
        match SettingsDb::open(data_dir) {
            Ok(db) => match db.load_into_settings() {
                Ok(settings) => {
                    log_diag("settings_db: boot: loaded existing settings.db");
                    return BootOutcome {
                        settings,
                        db: Some(Arc::new(db)),
                        source: BootSource::LoadedExistingDb,
                    };
                }
                Err(e) => {
                    // Codex P2 v10 (2026-05-14): Corrupted のときだけ bak 経路に倒す。
                    // Transient / Permission / Rusqlite (= 非破損) は quarantine しないで
                    // FailedFallbackDefault + save 抑止に倒す。誤って quarantine すると
                    // **正常な DB が `.corrupted-*` に飛ばされて永久に失われる**。
                    drop(db);
                    match e {
                        SettingsDbError::Corrupted(msg) => {
                            log_diag(&format!(
                                "settings_db: boot: load_into_settings reports Corrupted ({msg}); attempting bak recovery"
                            ));
                            return boot_recover_from_bak(data_dir);
                        }
                        other => {
                            log_diag(&format!(
                                "settings_db: boot: load_into_settings non-corruption failure ({other}); \
                                 suppressing save this session (DB left untouched)"
                            ));
                            return BootOutcome {
                                settings: Settings::default(),
                                db: None,
                                source: BootSource::FailedFallbackDefault,
                            };
                        }
                    }
                }
            },
            Err(SettingsDbError::Corrupted(msg)) => {
                log_diag(&format!(
                    "settings_db: boot: main DB corrupted ({msg}); attempting bak recovery"
                ));
                return boot_recover_from_bak(data_dir);
            }
            Err(SettingsDbError::Transient(e)) => {
                log_diag(&format!(
                    "settings_db: boot: main DB transient I/O failure after retries ({e}); \
                     suppressing save this session"
                ));
                return BootOutcome {
                    settings: Settings::default(),
                    db: None,
                    source: BootSource::FailedFallbackDefault,
                };
            }
            Err(SettingsDbError::Permission(e)) => {
                log_diag(&format!(
                    "settings_db: boot: permission error opening main DB ({e}); \
                     suppressing save this session"
                ));
                return BootOutcome {
                    settings: Settings::default(),
                    db: None,
                    source: BootSource::FailedFallbackDefault,
                };
            }
            Err(other) => {
                log_diag(&format!(
                    "settings_db: boot: unexpected open error: {other}"
                ));
                return BootOutcome {
                    settings: Settings::default(),
                    db: None,
                    source: BootSource::FailedFallbackDefault,
                };
            }
        }
    }

    // 2. family 不在: settings.json があれば migration。
    //    Codex P1 v8b (2026-05-14): per-file metadata だけだと AV / cloud sync 等の
    //    transient NotFound で「JSON 無し」と誤判定して旧 JSON migration を奪う事故が
    //    起きる。`legacy_json_family_exists` は per-file metadata + read_dir の二経路で
    //    robust 化されているのでそれを使う。
    let json_exists = crate::settings::legacy_json_family_exists(data_dir);
    if json_exists {
        match migrate_from_settings_json(data_dir) {
            Ok((db, settings)) => {
                log_diag("settings_db: boot: migrated from settings.json");
                return BootOutcome {
                    settings,
                    db: Some(Arc::new(db)),
                    source: BootSource::MigratedFromJson,
                };
            }
            Err(e) => {
                log_diag(&format!("settings_db: boot: JSON migration failed: {e}"));
                return BootOutcome {
                    settings: Settings::default(),
                    db: None,
                    source: BootSource::FailedFallbackDefault,
                };
            }
        }
    }

    // 3. 何もない: clean install
    match SettingsDb::create_new(data_dir) {
        Ok(db) => {
            let settings = Settings::default();
            if let Err(e) = db.save_full(&settings) {
                log_diag(&format!(
                    "settings_db: boot: clean install save_full failed: {e}; suppressing save"
                ));
                return BootOutcome {
                    settings,
                    db: None,
                    source: BootSource::FailedFallbackDefault,
                };
            }
            log_diag("settings_db: boot: clean install");
            BootOutcome {
                settings,
                db: Some(Arc::new(db)),
                source: BootSource::CleanInstall,
            }
        }
        Err(e) => {
            log_diag(&format!("settings_db: boot: clean install failed: {e}"));
            BootOutcome {
                settings: Settings::default(),
                db: None,
                source: BootSource::FailedFallbackDefault,
            }
        }
    }
}

/// 壊れた main DB の家族を quarantine し、bak1..bak10 を新しい順に試行して復旧する。
fn boot_recover_from_bak(data_dir: &Path) -> BootOutcome {
    let moved = quarantine_db_files(data_dir);
    log_diag(&format!(
        "settings_db: boot: quarantined {moved} file(s); now scanning bak1..bak10"
    ));
    for n in 1..=10 {
        let bak_name = format!("settings.db.bak{n}");
        let bak_path = data_dir.join(&bak_name);
        if !bak_path.exists() {
            continue;
        }
        // bak を main 位置に copy して open する (= rename だと bak が消えるが、再起動時に
        // 同じ bak をまた使いたい場合があるので copy で残す)。
        let main_path = settings_db_path(data_dir);
        if let Err(e) = std::fs::copy(&bak_path, &main_path) {
            log_diag(&format!(
                "settings_db: boot: failed to copy {bak_name} -> settings.db: {e}"
            ));
            continue;
        }
        log_diag(&format!(
            "settings_db: boot: restored from {bak_name}; opening"
        ));
        match SettingsDb::open(data_dir) {
            Ok(db) => match db.load_into_settings() {
                Ok(settings) => {
                    return BootOutcome {
                        settings,
                        db: Some(Arc::new(db)),
                        source: BootSource::RestoredFromDbBackup,
                    };
                }
                Err(e) => {
                    // Codex P2 v10 (2026-05-14): Corrupted のときだけ quarantine + 次の bak。
                    // Transient / Permission は I/O 状況が悪いだけで bak 自体は無事の可能性が
                    // 高いので bak chain を勝手に消費せず即座に save 抑止に倒す。
                    drop(db);
                    match e {
                        SettingsDbError::Corrupted(msg) => {
                            log_diag(&format!(
                                "settings_db: boot: restored {bak_name} loads as Corrupted ({msg}); \
                                 quarantining and trying next bak"
                            ));
                            quarantine_db_files(data_dir);
                            continue;
                        }
                        other => {
                            log_diag(&format!(
                                "settings_db: boot: restored {bak_name} loaded with non-corruption error ({other}); \
                                 aborting bak chain to preserve remaining backups"
                            ));
                            return BootOutcome {
                                settings: Settings::default(),
                                db: None,
                                source: BootSource::FailedFallbackDefault,
                            };
                        }
                    }
                }
            },
            Err(e) => {
                // Codex P2 v10 (2026-05-14): 同上。Corrupted のみ quarantine、その他は abort。
                match e {
                    SettingsDbError::Corrupted(msg) => {
                        log_diag(&format!(
                            "settings_db: boot: restored {bak_name} open Corrupted ({msg}); \
                             quarantining and trying next bak"
                        ));
                        quarantine_db_files(data_dir);
                        continue;
                    }
                    other => {
                        log_diag(&format!(
                            "settings_db: boot: restored {bak_name} open failed with non-corruption error ({other}); \
                             aborting bak chain to preserve remaining backups"
                        ));
                        return BootOutcome {
                            settings: Settings::default(),
                            db: None,
                            source: BootSource::FailedFallbackDefault,
                        };
                    }
                }
            }
        }
    }
    log_diag("settings_db: boot: bak recovery exhausted; suppressing save this session");
    BootOutcome {
        settings: Settings::default(),
        db: None,
        source: BootSource::FailedFallbackDefault,
    }
}

// ---------------------------------------------------------------------------
// lazy global (with_db / with_db_result)
// ---------------------------------------------------------------------------

static GLOBAL_DB: Mutex<Option<(PathBuf, Arc<SettingsDb>)>> = Mutex::new(None);

/// 本セッションの save を完全に抑止するフラグ (Codex P2 v8b-3 2026-05-14)。
///
/// `boot_settings_db` が `BootOutcome.db == None` を返すとき (= 全復旧経路が失敗)
/// にセットする。`with_db` はこのフラグが立っているとき `Transient` の代わりに
/// `Suppressed` を返し、lazy re-open を試みない。これで Phase 3 で
/// `Settings::save()` が誤って `with_db` 経由で defaults を書き戻す事故を防ぐ。
///
/// Phase 3 で `Settings::save()` 直接側にも対称的なフラグを設ける予定 (=
/// 旧 `MAIN_UNREADABLE_THIS_SESSION` のセマンティクス継承)。
static SAVE_SUPPRESSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 本セッションの save 抑止を強制する / 解除する。
pub fn set_save_suppressed(suppressed: bool) {
    SAVE_SUPPRESSED.store(suppressed, std::sync::atomic::Ordering::Relaxed);
}

pub fn save_suppressed() -> bool {
    SAVE_SUPPRESSED.load(std::sync::atomic::Ordering::Relaxed)
}

/// `boot_settings_db` から GLOBAL_DB をセット / クリアする。
///
/// `db == Some(arc)` → 次の `with_db` で arc を返す。save 抑止フラグもクリア。
/// `db == None` → save 抑止フラグを立てる (= `with_db` は `Suppressed` を返す)。
pub(crate) fn set_global_db(data_dir: &Path, db: Option<Arc<SettingsDb>>) {
    let suppress = db.is_none();
    if let Ok(mut guard) = GLOBAL_DB.lock() {
        *guard = db.map(|arc| (data_dir.to_path_buf(), arc));
    }
    set_save_suppressed(suppress);
}

/// グローバル `SettingsDb` ハンドルを使って closure を実行する。
///
/// **Phase 2 以降**: `boot_settings_db` が予め global を populate しているので、
/// `with_db` は populate された Arc を取り出すだけ。Boot 以前 (=旧 Phase 1 互換) や
/// test override で `data_dir::get()` が変わったケースでは、`SettingsDb::open()` を
/// lazy に試みる。open に失敗したら `Transient` 等が伝搬する。
///
/// **save 抑止状態** (Codex P2 v8b-3 2026-05-14): `SAVE_SUPPRESSED` が立っている間は
/// `SettingsDbError::SaveSuppressed` を返し、lazy re-open を試みない。これで
/// `boot_settings_db` が FailedFallbackDefault を返した後、Phase 3 caller が誤って
/// defaults を DB に書き戻すのを防ぐ。
///
/// - 内部 lock は `Arc<SettingsDb>` を clone する間だけ。closure 実行中は
///   global lock を持たない (= 並列 `with_db` がブロックしない)
pub fn with_db<R>(f: impl FnOnce(&SettingsDb) -> R) -> Result<R, SettingsDbError> {
    if save_suppressed() {
        return Err(SettingsDbError::SaveSuppressed);
    }
    let db_arc: Arc<SettingsDb> = {
        let mut guard = GLOBAL_DB.lock().map_err(|_| SettingsDbError::Poisoned)?;
        let current_dir = crate::data_dir::get();
        let need_reopen = guard.as_ref().map(|(d, _)| d) != Some(&current_dir);
        if need_reopen {
            let new_db = SettingsDb::open(&current_dir)?;
            *guard = Some((current_dir.clone(), Arc::new(new_db)));
        }
        // unwrap: 直前で必ず Some を入れている、または既に Some。
        Arc::clone(&guard.as_ref().expect("just set or already Some").1)
    };
    Ok(f(&db_arc))
}

/// `with_db` の variant。closure が `Result` を返すケースで nested Result を flatten する。
pub fn with_db_result<X>(
    f: impl FnOnce(&SettingsDb) -> Result<X, SettingsDbError>,
) -> Result<X, SettingsDbError> {
    with_db(f).and_then(std::convert::identity)
}

/// テスト用: グローバル DB ハンドルをクリアする。
///
/// `data_dir::set_test_override` で dir を切り替えた直後に呼ぶと、次の
/// `with_db` で確実に再 open される (= 前回テストの handle が残らない)。
#[cfg(test)]
pub fn reset_global_for_test() {
    if let Ok(mut guard) = GLOBAL_DB.lock() {
        *guard = None;
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{RecentApp, Settings, TagDef};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// data_dir override をテンポラリ dir にセットし、global DB handle もクリアする。
    /// 返値の guard が drop されると override が解除される。
    ///
    /// 直列化ロックは `crate::data_dir::test_override_lock()` (= process-global) を
    /// 使うので、settings.rs / app/tests.rs 等他ファイルのテストとも安全に
    /// インターロックする (Codex P2 v8b-5 2026-05-14)。
    struct DataDirOverrideGuard {
        _tempdir: TempDir,
        path: PathBuf,
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    impl DataDirOverrideGuard {
        fn new() -> Self {
            let serial = crate::data_dir::test_override_lock();
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();
            crate::data_dir::set_test_override(Some(path.clone()));
            reset_global_for_test();
            set_save_suppressed(false);
            Self {
                _tempdir: tempdir,
                path,
                _serial: serial,
            }
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for DataDirOverrideGuard {
        fn drop(&mut self) {
            crate::data_dir::set_test_override(None);
            reset_global_for_test();
            set_save_suppressed(false);
        }
    }

    fn sample_settings() -> Settings {
        let mut s = Settings::default();
        s.grid_cols = 7;
        s.thumb_quality = 88;
        s.last_folder = Some(PathBuf::from(r"C:\Users\test\Pictures"));
        s.favorites = vec![
            FavoriteEntry {
                id: Uuid::new_v4(),
                name: "Pics".to_string(),
                path: PathBuf::from(r"C:\Pics"),
                auto_index_structure: true,
                auto_index_metadata: false,
                auto_index_thumbs: true,
            },
            FavoriteEntry {
                id: Uuid::new_v4(),
                name: "Videos".to_string(),
                path: PathBuf::from(r"D:\Videos"),
                auto_index_structure: false,
                auto_index_metadata: true,
                auto_index_thumbs: false,
            },
        ];
        s.tags = vec![
            TagDef::new("原神".to_string()),
            TagDef::new("風景".to_string()),
        ];
        let mut resume = HashMap::new();
        resume.insert(r"C:\v\a.mp4".to_string(), 12.5);
        resume.insert(r"C:\v\b.mkv".to_string(), 3600.25);
        s.video_resume_positions = resume;
        s.recent_open_with_apps = vec![RecentApp {
            display_name: "Editor".to_string(),
            exe_path: r"C:\bin\edit.exe".to_string(),
        }];
        s.custom_open_with_apps = vec![RecentApp {
            display_name: "Photoshop".to_string(),
            exe_path: r"C:\bin\ps.exe".to_string(),
        }];
        s.vst3_plugins = vec![
            Vst3PluginEntry {
                path: r"C:\vst3\eq.vst3".to_string(),
                bypass: false,
                state: Some("AAAA".to_string()),
                user_hidden: false,
                gui_pos: Some((100, 200)),
                gui_size: Some((640, 480)),
            },
            Vst3PluginEntry {
                path: r"C:\vst3\comp.vst3".to_string(),
                bypass: true,
                state: None,
                user_hidden: true,
                gui_pos: None,
                gui_size: None,
            },
        ];
        s.vst3_chain_slots.slots[0] = Some(Vst3ChainPresetSlot {
            name: "Slot 1".to_string(),
            plugins: s.vst3_plugins.clone(),
            gui_visible: false,
            video_compact: true,
        });
        s.vst3_chain_slots.slots[5] = Some(Vst3ChainPresetSlot {
            name: "Slot 5".to_string(),
            plugins: vec![],
            gui_visible: true,
            video_compact: false,
        });
        s
    }

    fn assert_settings_eq(a: &Settings, b: &Settings) {
        // 大型構造体なのでフィールドごと比較はせず、serde の JSON で同等性を見る。
        let aj = serde_json::to_value(a).unwrap();
        let bj = serde_json::to_value(b).unwrap();
        assert_eq!(aj, bj, "Settings did not round-trip via SQLite");
    }

    #[test]
    fn roundtrip_default_settings() {
        let db = SettingsDb::open_in_memory_for_test().unwrap();
        let original = Settings::default();
        db.save_full(&original).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_settings_eq(&original, &loaded);
    }

    #[test]
    fn roundtrip_populated_settings() {
        let db = SettingsDb::open_in_memory_for_test().unwrap();
        let original = sample_settings();
        db.save_full(&original).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_settings_eq(&original, &loaded);
    }

    #[test]
    fn roundtrip_via_disk_file() {
        let dir = TempDir::new().unwrap();
        let original = sample_settings();
        {
            // 初回は bootstrap なので `create_new` (= CREATE フラグあり)。
            let db = SettingsDb::create_new(dir.path()).unwrap();
            db.save_full(&original).unwrap();
        }
        // 再 open して同一性を確認 (= WAL の checkpoint も含めて永続化されているか)。
        // ここは `open` (= CREATE なし) で十分 (= 既に file 存在)。
        let db2 = SettingsDb::open(dir.path()).unwrap();
        let loaded = db2.load_into_settings().unwrap();
        assert_settings_eq(&original, &loaded);
    }

    #[test]
    fn open_existing_fails_when_main_missing() {
        // Codex P2 v3 (2026-05-13): family が見える / 見えないに関わらず、main DB が
        // 存在しない状態で `SettingsDb::open()` を呼んでも **空の DB を作って成功させない**。
        // 必ず Transient で失敗し、上層の save 抑止経路に届くこと。
        let dir = TempDir::new().unwrap();
        let err = match SettingsDb::open(dir.path()) {
            Ok(_) => panic!("open() should not create a new DB when main is missing"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Transient(_)),
            "expected Transient (file missing), got: {err:?}"
        );
        // 副作用として settings.db を物理的に作っていないことを確認 (= CREATE フラグなし)。
        assert!(!dir.path().join("settings.db").exists());
    }

    #[test]
    fn create_new_creates_missing_db() {
        // `create_new` は CREATE フラグ付きなので、最初に呼んだときに DB を作る。
        // 続いて save_full を実行することで bootstrap_complete marker が書かれ、
        // 後続の `open` (= RequireExisting) が成功するようになる。
        let dir = TempDir::new().unwrap();
        {
            let db = SettingsDb::create_new(dir.path()).unwrap();
            db.save_full(&Settings::default()).unwrap();
        }
        assert!(dir.path().join("settings.db").exists());
        let _db2 = SettingsDb::open(dir.path()).unwrap();
    }

    #[test]
    fn favorites_order_preserved() {
        let db = SettingsDb::open_in_memory_for_test().unwrap();
        let mut s = Settings::default();
        s.favorites = (0..5)
            .map(|i| FavoriteEntry::new(format!("name{i}"), PathBuf::from(format!("C:\\p{i}"))))
            .collect();
        db.save_full(&s).unwrap();
        let loaded = db.load_into_settings().unwrap();
        let names: Vec<&str> = loaded.favorites.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["name0", "name1", "name2", "name3", "name4"]);
    }

    #[test]
    fn vst3_plugins_chain_index_unique() {
        // chain_index UNIQUE 制約が効いているか直接 SQL で確認。
        let db = SettingsDb::open_in_memory_for_test().unwrap();
        let inner = db.inner.lock().unwrap();
        let r = inner.conn.execute(
            "INSERT INTO vst3_plugins (plugin_path, chain_index) VALUES ('a', 0)",
            [],
        );
        assert!(r.is_ok());
        let r2 = inner.conn.execute(
            "INSERT INTO vst3_plugins (plugin_path, chain_index) VALUES ('b', 0)",
            [],
        );
        assert!(r2.is_err(), "duplicate chain_index should fail");
    }

    /// `vst3_plugins` に「sentinel」行を直挿入する。次に `save_full` が `DELETE+INSERT`
    /// を走らせれば消える。hash skip が効けば残る。これで「2 回目の save_full が VST3
    /// テーブルを実際に書き直したか」を観測する。
    fn inject_vst3_sentinel(db: &SettingsDb) {
        let inner = db.inner.lock().unwrap();
        inner
            .conn
            .execute(
                "INSERT INTO vst3_plugins (plugin_path, chain_index)
                 VALUES ('___sentinel___', 9999)",
                [],
            )
            .unwrap();
    }

    fn sentinel_present(db: &SettingsDb) -> bool {
        let inner = db.inner.lock().unwrap();
        let count: i64 = inner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM vst3_plugins WHERE plugin_path = '___sentinel___'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        count > 0
    }

    #[test]
    fn vst3_hash_skip_on_unchanged_save() {
        // 同じ Settings を 2 回保存したとき、2 回目の VST3 hash は一致するので
        // DELETE+INSERT がスキップされる。sentinel 行を入れて生き残るか確認する。
        let db = SettingsDb::open_in_memory_for_test().unwrap();
        let s = sample_settings();
        db.save_full(&s).unwrap();
        inject_vst3_sentinel(&db);
        assert!(sentinel_present(&db));
        db.save_full(&s).unwrap();
        assert!(
            sentinel_present(&db),
            "sentinel must survive: hash skip should have prevented DELETE+INSERT"
        );
    }

    #[test]
    fn vst3_hash_skip_invalidated_on_change() {
        // VST3 内容が変わったら DELETE+INSERT が走り、sentinel は消える。
        let db = SettingsDb::open_in_memory_for_test().unwrap();
        let mut s = sample_settings();
        db.save_full(&s).unwrap();
        inject_vst3_sentinel(&db);
        assert!(sentinel_present(&db));
        // bypass を反転して 2 回目を保存する → hash が変わる → DELETE+INSERT
        s.vst3_plugins[0].bypass = !s.vst3_plugins[0].bypass;
        db.save_full(&s).unwrap();
        assert!(
            !sentinel_present(&db),
            "sentinel must be gone: content changed should trigger DELETE+INSERT"
        );
    }

    #[test]
    fn vst3_slots_hash_skip_on_unchanged_save() {
        // chain と同じく、slots 側も hash skip が効くか sentinel で確認する。
        let db = SettingsDb::open_in_memory_for_test().unwrap();
        let s = sample_settings();
        db.save_full(&s).unwrap();
        // slot 用 sentinel: slot_index=99 を直挿入 (PRIMARY KEY なので衝突なし)。
        {
            let inner = db.inner.lock().unwrap();
            inner
                .conn
                .execute(
                    "INSERT INTO vst3_chain_slots
                        (slot_index, name, gui_visible, video_compact, plugins_json)
                     VALUES (99, '___sentinel___', 1, 0, '[]')",
                    [],
                )
                .unwrap();
        }
        db.save_full(&s).unwrap();
        let count: i64 = {
            let inner = db.inner.lock().unwrap();
            inner
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM vst3_chain_slots WHERE slot_index = 99",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(
            count, 1,
            "slots hash skip should prevent DELETE+INSERT on unchanged content"
        );
    }

    #[test]
    fn schema_meta_records_version() {
        let db = SettingsDb::open_in_memory_for_test().unwrap();
        let inner = db.inner.lock().unwrap();
        let v: String = inner
            .conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let app_v: String = inner
            .conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'app_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(app_v, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn nonfinite_resume_position_is_dropped() {
        // 直接 HashMap に NaN を入れたケースで save が失敗せず、その row が抜けるか。
        let db = SettingsDb::open_in_memory_for_test().unwrap();
        let mut s = Settings::default();
        let mut resume = HashMap::new();
        resume.insert("ok".to_string(), 10.0);
        resume.insert("bad".to_string(), f64::NAN);
        s.video_resume_positions = resume;
        db.save_full(&s).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_eq!(loaded.video_resume_positions.len(), 1);
        assert!(loaded.video_resume_positions.contains_key("ok"));
        assert!(!loaded.video_resume_positions.contains_key("bad"));
    }

    #[test]
    fn family_filename_classifier() {
        assert!(is_family_filename("settings.db"));
        assert!(is_family_filename("settings.db-wal"));
        assert!(is_family_filename("settings.db-shm"));
        assert!(is_family_filename("settings.db.bak1"));
        assert!(is_family_filename("settings.db.bak10"));
        assert!(!is_family_filename("settings.db.bak"));
        assert!(!is_family_filename("settings.db.bakX"));
        assert!(!is_family_filename("settings.db.foo"));
        assert!(!is_family_filename("settings.json"));
        // spec §6 範囲外 / 非正準形 (Codex P3 v4/v5 2026-05-13)。
        assert!(!is_family_filename("settings.db.bak0"));
        assert!(!is_family_filename("settings.db.bak11"));
        assert!(!is_family_filename("settings.db.bak100"));
        // 先頭ゼロは非正準なので弾く (Codex P3 v5)。
        assert!(!is_family_filename("settings.db.bak01"));
        assert!(!is_family_filename("settings.db.bak001"));
    }

    #[test]
    fn require_existing_rejects_empty_sqlite_db() {
        // Codex P3 v5 (2026-05-13): 0 バイトファイルや形式上 OK な空 DB を
        // RequireExisting で開いたとき、defaults でロードせず Corrupted にする。
        // これで上層が bak / JSON migration の fallback を試行できる。
        let dir = TempDir::new().unwrap();
        let main_path = dir.path().join("settings.db");

        // ケース 1: 完全に 0 バイトのファイル。SQLite 的には valid な空 DB と扱われる
        // (= integrity_check は ok を返す)。
        std::fs::File::create(&main_path).unwrap();
        let err = match SettingsDb::open(dir.path()) {
            Ok(_) => panic!("empty file should not pass RequireExisting"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "empty DB should be Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn require_existing_rejects_partial_init() {
        // ケース 2: schema_meta テーブルはあるが schema_version row が無い (= partial init)。
        let dir = TempDir::new().unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("settings.db")).unwrap();
        conn.execute_batch("CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        drop(conn);
        let err = match SettingsDb::open(dir.path()) {
            Ok(_) => panic!("partial init should not pass RequireExisting"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "partial init should be Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn create_new_refuses_when_only_bak_exists() {
        // Codex P2 v8 (2026-05-13): family pre-check。main DB が無くても、bak / WAL / SHM が
        // どれか 1 つでも見えるなら create_new は AlreadyBootstrapped で fail-fast する。
        // これで「main だけ transient で消えて、Phase 2 が clean install と誤判定」しても
        // bak chain が orphan 化せず生き残る。
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("settings.db.bak1"), b"dummy").unwrap();
        let err = match SettingsDb::create_new(dir.path()) {
            Ok(_) => panic!("create_new should refuse when bak1 is visible"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::AlreadyBootstrapped),
            "expected AlreadyBootstrapped, got: {err:?}"
        );
        // settings.db を **作らない** (= 既存の bak1 が clean install に巻き込まれない)。
        assert!(!dir.path().join("settings.db").exists());
    }

    #[test]
    fn corrupted_favorite_uuid_returns_corrupted() {
        // Codex P2 v8 (2026-05-13): row 内の UUID バイト長が 16 でない (= 破損) なら
        // silently nil-uuid に fallback せず Corrupted を返す。
        let dir = TempDir::new().unwrap();
        // bootstrap 済み DB を作る → ファイルを直接書き換えて UUID 列を 8 バイトに。
        {
            let db = SettingsDb::create_new(dir.path()).unwrap();
            let mut s = Settings::default();
            s.favorites
                .push(FavoriteEntry::new("name".into(), PathBuf::from(r"C:\\X")));
            db.save_full(&s).unwrap();
        }
        // 中身を壊す: id を 8 バイトに置換。
        let conn = rusqlite::Connection::open(dir.path().join("settings.db")).unwrap();
        conn.execute("UPDATE favorites SET id = X'0102030405060708' WHERE 1", [])
            .unwrap();
        drop(conn);
        let db = SettingsDb::open(dir.path()).unwrap();
        let err = match db.load_into_settings() {
            Ok(_) => panic!("corrupted UUID should fail load_into_settings"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "expected Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn require_existing_rejects_missing_table() {
        // Codex P2 v11 (2026-05-14): bootstrap 済み DB で必須テーブルの 1 つが消えていたら
        // Corrupted。init_schema が CREATE IF NOT EXISTS で silently 再作成する経路を塞ぐ。
        let dir = TempDir::new().unwrap();
        {
            let db = SettingsDb::create_new(dir.path()).unwrap();
            db.save_full(&Settings::default()).unwrap();
        }
        // 直接 SQL で `favorites` テーブルを drop。
        let conn = rusqlite::Connection::open(dir.path().join("settings.db")).unwrap();
        conn.execute_batch("DROP TABLE favorites;").unwrap();
        drop(conn);
        let err = match SettingsDb::open(dir.path()) {
            Ok(_) => panic!("missing favorites table should fail RequireExisting"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "expected Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn corrupted_settings_kv_value_returns_corrupted() {
        // Codex P2 v10 (2026-05-14): settings_kv の value 文字列が JSON として
        // parse できなければ Corrupted を返す。
        let dir = TempDir::new().unwrap();
        {
            let db = SettingsDb::create_new(dir.path()).unwrap();
            db.save_full(&Settings::default()).unwrap();
        }
        let conn = rusqlite::Connection::open(dir.path().join("settings.db")).unwrap();
        // settings_kv に壊れた行を 1 つ追加 (= 既存の任意 key を壊すと parse 経路に乗る)。
        conn.execute(
            "INSERT INTO settings_kv (key, value) VALUES ('__broken__', 'not json')",
            [],
        )
        .unwrap();
        drop(conn);
        let db = SettingsDb::open(dir.path()).unwrap();
        let err = match db.load_into_settings() {
            Ok(_) => panic!("malformed settings_kv value should fail load"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "expected Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn settings_kv_shape_mismatch_returns_corrupted() {
        // Codex P2 v10 (2026-05-14): settings_kv の値は valid JSON だが型が違う
        // (grid_cols が文字列など) ケース。最終 from_value が Serde で落ちる経路を
        // Corrupted に統一する。
        let dir = TempDir::new().unwrap();
        {
            let db = SettingsDb::create_new(dir.path()).unwrap();
            db.save_full(&Settings::default()).unwrap();
        }
        let conn = rusqlite::Connection::open(dir.path().join("settings.db")).unwrap();
        // grid_cols は usize として load される。"foo" 文字列だと from_value が落ちる。
        conn.execute(
            "UPDATE settings_kv SET value = '\"foo\"' WHERE key = 'grid_cols'",
            [],
        )
        .unwrap();
        drop(conn);
        let db = SettingsDb::open(dir.path()).unwrap();
        let err = match db.load_into_settings() {
            Ok(_) => panic!("shape mismatch should fail load"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "expected Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn out_of_range_vst3_slot_index_returns_corrupted() {
        // Codex P2 v9 (2026-05-14): vst3_chain_slots.slot_index が 0..=9 を外れた値で
        // 入っていたら、silently skip せず Corrupted を返す (= 次の save_full で永久消失
        // する隙を作らない)。
        let dir = TempDir::new().unwrap();
        {
            let db = SettingsDb::create_new(dir.path()).unwrap();
            db.save_full(&Settings::default()).unwrap();
        }
        let conn = rusqlite::Connection::open(dir.path().join("settings.db")).unwrap();
        conn.execute(
            "INSERT INTO vst3_chain_slots
                (slot_index, name, gui_visible, video_compact, plugins_json)
             VALUES (42, 'oops', 1, 0, '[]')",
            [],
        )
        .unwrap();
        drop(conn);
        let db = SettingsDb::open(dir.path()).unwrap();
        let err = match db.load_into_settings() {
            Ok(_) => panic!("out-of-range slot_index should fail load_into_settings"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "expected Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn corrupted_vst3_slot_plugins_json_returns_corrupted() {
        // 同上 (Codex P2 v8): plugins_json が壊れていたら Corrupted を返す。
        let dir = TempDir::new().unwrap();
        {
            let db = SettingsDb::create_new(dir.path()).unwrap();
            let mut s = Settings::default();
            s.vst3_chain_slots.slots[0] = Some(Vst3ChainPresetSlot {
                name: "slot".into(),
                plugins: vec![],
                gui_visible: true,
                video_compact: false,
            });
            db.save_full(&s).unwrap();
        }
        let conn = rusqlite::Connection::open(dir.path().join("settings.db")).unwrap();
        conn.execute(
            "UPDATE vst3_chain_slots SET plugins_json = 'NOT JSON' WHERE slot_index = 0",
            [],
        )
        .unwrap();
        drop(conn);
        let db = SettingsDb::open(dir.path()).unwrap();
        let err = match db.load_into_settings() {
            Ok(_) => panic!("corrupted plugins_json should fail load_into_settings"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "expected Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn create_new_refuses_already_bootstrapped_db() {
        // Codex P2 v7 (2026-05-13): family が transient で見えなかったケースで
        // Phase 2 が誤って clean-install 経路を選び `create_new` を呼んでも、
        // 既存 DB が bootstrap_complete を持っていれば即座に AlreadyBootstrapped で
        // 失敗し、続く save_full でユーザー設定が上書き全消去されるのを防ぐ。
        let dir = TempDir::new().unwrap();
        // 既存 bootstrapped DB を作る。
        {
            let db = SettingsDb::create_new(dir.path()).unwrap();
            let mut s = Settings::default();
            s.grid_cols = 12; // ユーザー設定だと識別できるよう特殊値
            db.save_full(&s).unwrap();
        }
        // ここで再度 `create_new` を呼ぶ → AlreadyBootstrapped。
        let err = match SettingsDb::create_new(dir.path()) {
            Ok(_) => panic!("create_new on bootstrapped DB should fail"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::AlreadyBootstrapped),
            "expected AlreadyBootstrapped, got: {err:?}"
        );
        // 既存内容が無事であることを確認 (= `open` で読めて grid_cols = 12 のまま)。
        let db = SettingsDb::open(dir.path()).unwrap();
        let loaded = db.load_into_settings().unwrap();
        assert_eq!(
            loaded.grid_cols, 12,
            "user settings must survive the refused create_new"
        );
    }

    #[test]
    fn require_existing_rejects_init_without_save() {
        // Codex P1 v6 (2026-05-13): create_new で init_schema は走ったが、最初の
        // save_full が呼ばれる前に crash 等で終了した状態。schema_version はあるが
        // bootstrap_complete marker は無い。RequireExisting は Corrupted にすべき。
        //
        // 復旧経路 (Phase 2): open() が Corrupted を返したら、ファイルを quarantine
        // (= 物理的に rename / delete) して bak chain に倒す or 再度 create_new。
        // 本テストは「open が Corrupted を返す」までで終わらせる (= recovery 部分は
        // Phase 2 で family pre-check 経由になるため Phase 1 の単体テスト範囲外)。
        let dir = TempDir::new().unwrap();
        // create_new は init_schema を走らせて schema_version を書くが、
        // save_full を呼ばないまま drop すれば bootstrap_complete は付かない。
        {
            let _db = SettingsDb::create_new(dir.path()).unwrap();
            // 意図的に save_full しない。
        }
        // settings.db ファイルは存在する。
        assert!(dir.path().join("settings.db").exists());
        let err = match SettingsDb::open(dir.path()) {
            Ok(_) => panic!("init-without-save should not pass RequireExisting"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "init-without-save should be Corrupted, got: {err:?}"
        );
        // 復旧: 残骸を物理削除 → create_new + save_full で再 bootstrap → open 成功。
        std::fs::remove_file(dir.path().join("settings.db")).unwrap();
        {
            let db = SettingsDb::create_new(dir.path()).unwrap();
            db.save_full(&Settings::default()).unwrap();
        }
        let _ok = SettingsDb::open(dir.path()).expect("after rebootstrap, open should succeed");
    }

    #[test]
    fn family_exists_detects_main_db() {
        let dir = TempDir::new().unwrap();
        assert!(!settings_db_family_exists(dir.path()));
        std::fs::write(dir.path().join("settings.db"), b"x").unwrap();
        assert!(settings_db_family_exists(dir.path()));
    }

    #[test]
    fn family_exists_detects_bak_only() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("settings.db.bak3"), b"x").unwrap();
        assert!(settings_db_family_exists(dir.path()));
    }

    #[test]
    fn backup_to_creates_snapshot() {
        // `sample_settings()` は呼ぶたびに新規 UUID を振るので、save 用と比較用で
        // 1 回だけ作って使い回す。
        let original = sample_settings();
        let dir = TempDir::new().unwrap();
        let db = SettingsDb::create_new(dir.path()).unwrap();
        db.save_full(&original).unwrap();
        let target = dir.path().join("settings.db.bak1");
        db.backup_to(&target).unwrap();
        assert!(target.exists());
        // snapshot 単独で再 open して中身を確認する。
        // VACUUM INTO は独立した DB ファイルを作るので、別 dir に移してから開く。
        let other_dir = TempDir::new().unwrap();
        let bak_in_other = other_dir.path().join("settings.db");
        std::fs::copy(&target, &bak_in_other).unwrap();
        let restored = SettingsDb::open(other_dir.path()).unwrap();
        let restored_settings = restored.load_into_settings().unwrap();
        assert_settings_eq(&original, &restored_settings);
    }

    #[test]
    fn open_corrupted_file_reports_corrupted() {
        // Codex P1 (2026-05-13) 対応: open / PRAGMA / integrity_check / init_schema の
        // どの段階で NotADatabase が surface しても `Corrupted` に分類されるべき。
        // `Rusqlite` や `Transient` で逃げる挙動は許容しない。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.db");
        // 「SQLite ヘッダではない」16 バイトを書く。
        std::fs::write(&path, b"NOT A SQLITE DB!").unwrap();
        let err = match SettingsDb::open(dir.path()) {
            Ok(_) => panic!("should fail to open a non-sqlite file"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "expected Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn classify_open_error_categories() {
        // Codex P1 (2026-05-13) 補強: 分類関数の挙動が spec §5.1 と一致するか直接確認する。
        fn make_err(code: rusqlite::ErrorCode) -> rusqlite::Error {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code,
                    extended_code: 0,
                },
                None,
            )
        }
        use rusqlite::ErrorCode::*;
        assert_eq!(
            classify_open_error(&make_err(NotADatabase)),
            OpenFailureKind::Corrupted
        );
        assert_eq!(
            classify_open_error(&make_err(DatabaseCorrupt)),
            OpenFailureKind::Corrupted
        );
        assert_eq!(
            classify_open_error(&make_err(DatabaseBusy)),
            OpenFailureKind::Transient
        );
        assert_eq!(
            classify_open_error(&make_err(DatabaseLocked)),
            OpenFailureKind::Transient
        );
        assert_eq!(
            classify_open_error(&make_err(CannotOpen)),
            OpenFailureKind::Transient
        );
        assert_eq!(
            classify_open_error(&make_err(SystemIoFailure)),
            OpenFailureKind::Transient
        );
        assert_eq!(
            classify_open_error(&make_err(PermissionDenied)),
            OpenFailureKind::Permission
        );
        assert_eq!(
            classify_open_error(&make_err(ReadOnly)),
            OpenFailureKind::Permission
        );
        // Non-SqliteFailure variants -> Other (= retry 候補にする側)
        assert_eq!(
            classify_open_error(&rusqlite::Error::QueryReturnedNoRows),
            OpenFailureKind::Other
        );
        // 未知の SqliteFailure code は Other に落ち、open 経路で Transient へ昇格する
        // (Codex P3 2026-05-13: spec §5.1 のテーブル「default」行のカバレッジ)。
        let unknown = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ApiMisuse,
                extended_code: 0,
            },
            None,
        );
        assert_eq!(classify_open_error(&unknown), OpenFailureKind::Other);
        // classify_rusqlite_error_for_open は Other を Transient に昇格させる。
        let err = classify_rusqlite_error_for_open(
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: ApiMisuse,
                    extended_code: 0,
                },
                None,
            ),
            "test",
        );
        assert!(
            matches!(err, SettingsDbError::Transient(_)),
            "unknown sqlite code should be promoted to Transient: {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 2: migration / quarantine / boot decision tree
    //
    // 以下のテストは `data_dir::set_test_override` を使うので、`data_dir_serial()` で
    // 直列化する。プロセス内で同時 1 本だけ走る前提。
    // -----------------------------------------------------------------------

    /// `data_dir` 配下に JSON settings ファイルを書く小さなヘルパ。
    fn write_legacy_json(dir: &Path, name: &str, settings: &Settings) {
        let json = serde_json::to_string_pretty(settings).unwrap();
        std::fs::write(dir.join(name), json).unwrap();
    }

    #[test]
    fn quarantine_db_files_renames_family() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        std::fs::write(dir.join("settings.db"), b"data").unwrap();
        std::fs::write(dir.join("settings.db-wal"), b"wal").unwrap();
        std::fs::write(dir.join("settings.db-shm"), b"shm").unwrap();
        let moved = quarantine_db_files(dir);
        assert_eq!(moved, 3);
        assert!(!dir.join("settings.db").exists());
        assert!(!dir.join("settings.db-wal").exists());
        assert!(!dir.join("settings.db-shm").exists());
        // .corrupted-<ts> が 3 つ生まれているか read_dir で確認。
        let mut corrupted = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".corrupted-") {
                corrupted += 1;
            }
        }
        assert_eq!(corrupted, 3);
    }

    #[test]
    fn quarantine_db_files_handles_missing() {
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        // 空 dir で呼んでも error にせず 0 を返すこと。
        let moved = quarantine_db_files(dir);
        assert_eq!(moved, 0);
    }

    #[test]
    fn migrate_from_settings_json_roundtrip() {
        // Phase 2 のコア integration test: 旧 settings.json を作って migration を回し、
        // settings.db に同等内容が入ること + 旧 JSON が .migrated-<ts> にリネームされること。
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        let mut original = Settings::default();
        original.grid_cols = 9;
        original.thumb_quality = 77;
        original
            .favorites
            .push(FavoriteEntry::new("Pics".into(), PathBuf::from(r"C:\Pics")));
        original.tags.push(TagDef::new("原神".into()));
        write_legacy_json(dir, "settings.json", &original);
        // bak1 も置いて読み比較で main 優先を確認 (= main があれば bak は読まない)。
        let mut bak = original.clone();
        bak.grid_cols = 999;
        write_legacy_json(dir, "settings.json.bak1", &bak);

        let (db, loaded) = migrate_from_settings_json(dir).expect("migration should succeed");
        // 値ベースで一致確認。
        assert_eq!(loaded.grid_cols, 9);
        assert_eq!(loaded.thumb_quality, 77);
        assert_eq!(loaded.favorites.len(), 1);
        assert_eq!(loaded.tags.len(), 1);
        // settings.db が物理的に存在し、再 open で同じ内容が出てくる。
        assert!(dir.join("settings.db").exists());
        let reloaded = db.load_into_settings().unwrap();
        assert_eq!(reloaded.grid_cols, 9);
        // 旧 JSON ファイルは .migrated-<ts> にリネームされて消えている。
        assert!(!dir.join("settings.json").exists());
        assert!(!dir.join("settings.json.bak1").exists());
        let mut migrated_files = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".migrated-") {
                migrated_files += 1;
            }
        }
        assert_eq!(migrated_files, 2, "main + bak1 should be migrated");

        // schema_meta に migrated_from_json_at が記録されている。
        let inner = db.inner.lock().unwrap();
        let ts: String = inner
            .conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'migrated_from_json_at'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!ts.is_empty(), "migrated_from_json_at must be set");
    }

    #[test]
    fn migrate_from_settings_json_uses_bak_when_main_missing() {
        // settings.json は無いが bak1 だけ残っているケース。
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        let mut original = Settings::default();
        original.grid_cols = 5;
        write_legacy_json(dir, "settings.json.bak1", &original);
        let (_db, loaded) = migrate_from_settings_json(dir).expect("bak migration should work");
        assert_eq!(loaded.grid_cols, 5);
        assert!(!dir.join("settings.json.bak1").exists());
    }

    #[test]
    fn migrate_from_settings_json_no_json_returns_corrupted() {
        // JSON が一切無い dir では migration は失敗する (= caller は clean install に倒す)。
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        let err = match migrate_from_settings_json(dir) {
            Ok(_) => panic!("expected migration failure on empty dir"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SettingsDbError::Corrupted(_)),
            "no JSON should map to Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn boot_clean_install() {
        let guard = DataDirOverrideGuard::new();
        let outcome = boot_settings_db(guard.path());
        assert_eq!(outcome.source, BootSource::CleanInstall);
        assert!(outcome.db.is_some());
        // 続けて with_db 経由でアクセスできる (= GLOBAL_DB が populate されている)。
        with_db(|db| {
            let _ = db.load_into_settings().unwrap();
        })
        .unwrap();
    }

    #[test]
    fn boot_loads_existing_db() {
        let guard = DataDirOverrideGuard::new();
        let mut s = Settings::default();
        s.grid_cols = 6;
        {
            let db = SettingsDb::create_new(guard.path()).unwrap();
            db.save_full(&s).unwrap();
        }
        let outcome = boot_settings_db(guard.path());
        assert_eq!(outcome.source, BootSource::LoadedExistingDb);
        assert_eq!(outcome.settings.grid_cols, 6);
    }

    #[test]
    fn boot_migrates_from_json() {
        let guard = DataDirOverrideGuard::new();
        let mut s = Settings::default();
        s.grid_cols = 11;
        write_legacy_json(guard.path(), "settings.json", &s);
        let outcome = boot_settings_db(guard.path());
        assert_eq!(outcome.source, BootSource::MigratedFromJson);
        assert_eq!(outcome.settings.grid_cols, 11);
        assert!(!guard.path().join("settings.json").exists());
        assert!(guard.path().join("settings.db").exists());
    }

    #[test]
    fn boot_restores_from_bak_when_main_corrupted() {
        // bootstrapped DB を作ってから main を壊し、bak1 を別 dir で create_new + save 経由で
        // 用意して、boot が bak から復旧することを確認する。
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        // 1) main を作って save (bootstrap_complete を立てる)
        let mut s = Settings::default();
        s.grid_cols = 21;
        {
            let db = SettingsDb::create_new(dir).unwrap();
            db.save_full(&s).unwrap();
            // 2) bak1 として VACUUM INTO で snapshot を取る
            db.backup_to(&dir.join("settings.db.bak1")).unwrap();
        }
        // 3) main を壊す (= NotADatabase になる程度に上書き)。
        //    WAL / SHM が残っていると open 時に「main は壊れているが WAL から戻る」可能性が
        //    あるため、まとめて削除しておく。
        std::fs::remove_file(dir.join("settings.db")).unwrap();
        std::fs::write(dir.join("settings.db"), b"GARBAGE-NOT-A-SQLITE-DB-CONTENT").unwrap();
        let _ = std::fs::remove_file(dir.join("settings.db-wal"));
        let _ = std::fs::remove_file(dir.join("settings.db-shm"));
        // 4) boot を回す。Corrupted 検出 → quarantine → bak1 から copy → open 成功。
        let outcome = boot_settings_db(dir);
        assert_eq!(outcome.source, BootSource::RestoredFromDbBackup);
        assert_eq!(outcome.settings.grid_cols, 21);
    }

    #[test]
    fn boot_failed_returns_default_with_suppress() {
        // 復旧の bak も json も無い + DB が transient で開けない状況をシミュレートする。
        // 簡単に作れるシナリオ: 「main は壊れている、bak も無い、json も無い」。
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        std::fs::write(dir.join("settings.db"), b"GARBAGE").unwrap();
        let outcome = boot_settings_db(dir);
        assert_eq!(outcome.source, BootSource::FailedFallbackDefault);
        assert!(outcome.db.is_none());
        // SAVE_SUPPRESSED が立っており、後続の with_db は SaveSuppressed で fail-fast する
        // (Codex P2 v8b-3 2026-05-14)。
        assert!(save_suppressed());
        let err = with_db(|_db| ()).expect_err("with_db should be suppressed");
        assert!(
            matches!(err, SettingsDbError::SaveSuppressed),
            "expected SaveSuppressed, got: {err:?}"
        );
    }

    #[test]
    fn boot_clears_save_suppressed_on_success() {
        // 成功 boot の後は save 抑止が解除されていること (= 直前のテストの状態が残らない)。
        let guard = DataDirOverrideGuard::new();
        // 強制的に suppressed 状態を作る → boot が成功して解除されるか確認。
        set_save_suppressed(true);
        let outcome = boot_settings_db(guard.path());
        assert_eq!(outcome.source, BootSource::CleanInstall);
        assert!(!save_suppressed());
        with_db(|_db| ()).expect("with_db should succeed after a fresh boot");
    }

    #[test]
    fn migrate_applies_load_time_migrations() {
        // Codex P2 v8b-1 (2026-05-14): nil-UUID favorite が複数あっても save_full が
        // PRIMARY KEY 衝突しない (= sanitize で個別 UUID が振られる)。
        let guard = DataDirOverrideGuard::new();
        let dir = guard.path();
        // 旧形式の favorite (= 文字列で path だけ書く) を 2 つ持つ JSON を用意。
        // Settings の Deserialize 実装が Legacy 形式を受け付け、id = Uuid::nil() として
        // load される (sanitize 前)。
        let raw = r#"{
            "favorites": ["C:\\A", "C:\\B"],
            "vst3_plugin_path": "C:\\X.vst3",
            "vst3_plugin_state": "AAA"
        }"#;
        std::fs::write(dir.join("settings.json"), raw).unwrap();
        let (_, loaded) =
            migrate_from_settings_json(dir).expect("migration should succeed with sanitize");
        assert_eq!(loaded.favorites.len(), 2);
        // 両方の id が nil でなく、かつ異なることを確認 (sanitize で新規 UUID 発行済み)。
        assert!(!loaded.favorites[0].id.is_nil());
        assert!(!loaded.favorites[1].id.is_nil());
        assert_ne!(loaded.favorites[0].id, loaded.favorites[1].id);
        // vst3 legacy が Vec に流れていること。
        assert_eq!(loaded.vst3_plugins.len(), 1);
        assert_eq!(loaded.vst3_plugin_path, None);
        assert_eq!(loaded.vst3_plugin_state, None);
    }
}
