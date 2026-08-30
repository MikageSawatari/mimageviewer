# 設定永続化 SQLite 移行計画

`%APPDATA%\mimageviewer\settings.json` をベースにした現行の設定永続化を、SQLite (`settings.db`)
に移行する。本ドキュメントは仕様確定版で、Phase 0 から実装着手するための spec。

## ステータス (2026-05-14)

**Phase 0-7 実装完了**。Codex review 計 27 ラウンド通過。894 unit tests pass / 13 ignored
(うち 12 は旧 JSON 経路の `#[ignore]` テスト、1 は SQLite 移行とは無関係の post_filter
性能テスト)。

| Phase | コミット数 | Codex round | 主成果物 |
|---|---|---|---|
| 0 | 1 | - | `MIV_SETTINGS_SAVE_TRACE` 計装 (Phase 6 で削除済み) |
| 1 | 7 | 12 | `SettingsDb` 基本機能 (open / load_into_settings / save_full / backup_to / lazy global)、29 unit tests |
| 2 | 5 | 6 | `migrate_from_settings_json` / `quarantine_db_files` / `boot_settings_db` 決定木、SAVE_SUPPRESSED フラグ、12 tests |
| 3 | 3 | 3 | `Settings::load` / `save` を SettingsDb 経由に rewire、`SettingsDb::rotate_backups`、`save_internal_no_rotation` |
| 4 | 4 | 4 | `folder_tree::FolderTreeOptions`、`susie_loader::init_pool` (2 段階初期化 + 世代カウンタ)、`App::new_from_settings`、並列 `Settings::load()` を main 1 箇所に集約 |
| 5+6 | 1 | 1 | spec §5 backup migration 確認、Phase 0 計装削除 |
| 7 | 1 | 1 | ドキュメント更新 (本書 + CLAUDE.md + docs/architecture-overview.md + README.md) |

実装ハイライト:
- ファイル単体の transient I/O 失敗で「全 NotFound → defaults 上書き」する事故を構造的に排除
- main NotFound は `read_dir` で本当に不在か再確認 (transient なら abort + 保護)
- create_new は family 可視時 `AlreadyBootstrapped` で fail-fast (= clean install 誤判定での bak 上書き防止)
- corrupt rows (UUID / plugins_json / out-of-range slot / settings_kv shape) は silently 戻さず `Corrupted` で上層に fallback を委ねる
- VST3 大型 row は hash skip で 7MB 級の重複 write を排除
- `bootstrap_complete` marker で「init_schema 後 / save_full 前に crash」した中身ゼロの DB を Corrupted として bak に倒す
- Susie pool は init/reload 世代カウンタで stale build が user choice を上書きしないようガード

2026-06-12 追記:
- タグカタログ再設計に合わせて `tags` テーブルへ `tag_key TEXT` と
  `show_shortcut INTEGER NOT NULL DEFAULT 1` を追加。
- 旧 `id/name/sort_index` 行は `normalize_tag_key(name)` で `tag_key` を補完し、
  同一 `tag_key` は最小 `sort_index` の行へ統合する。既存タグは後方互換として
  `show_shortcut=1` (= メニュー/ツールバーにピン留め表示)。
- `TagDef` は `{ id, tag_key, name, show_shortcut }` になり、`FacetFilter.tags` は
  表示文字列 `#タグ` ではなく `tag_key` を永続化する。

将来の delete (本ロードマップ外):
- **旧 `*.json` save 経路の物理削除**: spec §9 Phase 6 で「数バージョン後に」と明記。
  現状は `try_load_with_recovery` / `rotate_backups` / `write_atomic` / `quarantine_path` /
  `preupgrade_path` / `LoadOutcome` / `Settings::settings_path` / `log_disk_snapshot` /
  `log_one_file_snapshot` / `any_settings_file_exists` を `#[allow(dead_code)]` で残置。
  数バージョン後 (= 旧 JSON → SQLite 移行を経験したユーザーの settings.db が安定したことを
  確認してから) `.migrated-<ts>` リネーム経路ごと撤去する。
- **Phase 0 計装の再投入 (hot-path upsert API)**: Phase 0 の `MIV_SETTINGS_SAVE_TRACE`
  実測結果を踏まえ、`upsert_video_resume_position` 等の差分 row write API を追加するかを
  別途検討する (Phase 6 で計装は一旦撤去済み、必要なら再導入)。

## 1. 動機

### 1.1 観測された失敗モード

`settings.log` (= `%APPDATA%\mimageviewer\logs\settings.log`、`settings_diag_log` の出力先) に
過去 1 ヶ月で **94 回**、以下のパターンが記録されている:

```
[ts] settings:   main settings.json: missing
[ts] settings:   bak1: missing
...
[ts] settings:   bak10: missing
[ts] settings: no readable settings/backup found; using built-in default
[ts] settings: save ok: bytes=4703 favorites=0 rotated=false
```

物理的に存在する 11 ファイル全部に対して `std::fs::read` / `std::fs::metadata` が
同時に `NotFound` を返す現象が間欠的に発生。2026-05-12 / 2026-05-13 にユーザー設定
(14 favorites、176 video resume positions、~7MB の VST3 state) が defaults で
上書きされる事故が再発。

### 1.2 構造的な脆弱性

1. **`write_atomic` の race window** ([data_dir.rs:116-122](../src/data_dir.rs)): `remove_file → rename`
   の間に settings.json が物理的に存在しない瞬間がある。`rename` は Windows でも `MOVEFILE_REPLACE_EXISTING`
   で既存上書きアトミックなので、`remove_file` は本来不要。
2. **`Settings::load()` が並列に呼ばれる** ([main.rs:612](../src/main.rs) + susie-init thread の
   [susie_loader.rs:441](../src/susie_loader.rs))。
3. **`Settings::load()` がセッション中に何度も呼ばれる** ([folder_tree.rs:330](../src/folder_tree.rs)
   はフォルダ移動のたびに発火)。
4. **Default fallback → migration save の自動発火** ([settings.rs:1969-1977](../src/settings.rs)):
   `version_changed=true` で `save()` が走り、defaults を settings.json に書き戻す。**ここが corruption の
   実害発生源**。
5. **2026-05-12 の防御 (`MAIN_UNREADABLE_THIS_SESSION`) が単一 `metadata()` に依存** ([settings.rs:1743](../src/settings.rs))。
   `metadata()` も transient で NotFound を返す今回のケースには無力 (= 過去 1 ヶ月で防御発動 0 回)。

### 1.3 I/O 浪費

VST3 state が settings.json の **99.7%** (3.7MB + 3.5MB = 7.2MB) を占める。`Settings::save()` は毎回
全文を `write_atomic` で書き直すため、video 再生位置 / window 位置 / 列数変更等の小さな変更でも
**毎回 7MB 級の write** が発生していた (なお、現在のコードでは 5 秒ごとの自動 save は既に消えている
可能性あり — Phase 0 で実測確認する)。

### 1.4 SQLite で消える失敗モード

| 失敗モード | 現状 | SQLite 化後 |
|---|---|---|
| `write_atomic` の remove + rename gap | あり | **なし** (in-place page update) |
| 起動後の再 load で transient NotFound | あり (folder_tree, susie 等) | **なし** (open handle 経由のクエリ) |
| 並列 `Settings::load()` race | あり | **なし** (lazy global で 1 connection) |
| 世代 rotation 中の per-file rename race | あり | **なし** (backup = `VACUUM INTO`) |
| AV / cloud sync が settings.json を一瞬隠す | 影響大 | 影響少 (handle 維持) |
| 7MB 全文 rewrite | あり | **なし** (差分 row UPDATE + VST3 hash skip) |

## 2. 目標 / 非目標

### 目標

- 既存 `Settings` 構造体 (フィールド・型・Default) は維持
- 既存 100+ 箇所の `self.settings.foo = X` / `self.settings.save()` 呼び出しに変更なし
- セッション中の per-load transient I/O 失敗で設定が消える事故を構造的に排除
- VST3 state を含む大型 row の冗長 write を排除

### 非目標

- 設定の手動 JSON 編集サポート (= 移行で消える、許容)
- 複数プロセス同時起動サポート (= 現状もサポート外)
- 設定の暗号化 / 認証 (現状の plain JSON と同じ saturation)
- 既存 SQLite DB 群 (catalog / rating / rotation 等) との connection 統一 (optional)

## 3. アーキテクチャ

### 3.1 SettingsDb 層を追加

```rust
// src/settings_db.rs (新規)

use std::sync::{Arc, Mutex};
use std::path::{Path, PathBuf};
use rusqlite::Connection;

pub struct SettingsDb {
    inner: Mutex<Inner>,
}

struct Inner {
    conn: Connection,
    // VST3 大型 row の dirty 検出用、commit 成功時のみ更新する。
    // None = 未確認、Some(h) = 直近の commit 成功時の hash。
    last_saved_vst3_chain_hash: Option<u64>,
    last_saved_vst3_slots_hash: Option<u64>,
}
```

### 3.2 lazy global

```rust
static GLOBAL_DB: Mutex<Option<(PathBuf, Arc<SettingsDb>)>> = Mutex::new(None);

/// closure は global lock の外で実行される。lock 中は Arc clone のみ。
pub fn with_db<R>(f: impl FnOnce(&SettingsDb) -> R) -> Result<R, SettingsDbError> {
    let db_arc: Arc<SettingsDb> = {
        let mut guard = GLOBAL_DB.lock().map_err(|_| SettingsDbError::Poisoned)?;
        let current_dir = crate::data_dir::get();
        let need_reopen = guard.as_ref().map(|(d, _)| d) != Some(&current_dir);
        if need_reopen {
            let new_db = SettingsDb::open(&current_dir)?;
            *guard = Some((current_dir.clone(), Arc::new(new_db)));
        }
        Arc::clone(&guard.as_ref().unwrap().1)
    };
    // ここで lock 解放
    Ok(f(&db_arc))
}

/// closure が Result を返す場合の helper。nested Result を flatten する。
pub fn with_db_result<X>(
    f: impl FnOnce(&SettingsDb) -> Result<X, SettingsDbError>,
) -> Result<X, SettingsDbError> {
    with_db(f).and_then(std::convert::identity)
}
```

`data_dir::get()` が test override で変わったら自動的に re-open する設計。テスト側は
`data_dir::set_test_override(Some(tempdir))` を呼ぶだけで切替できる (= `reset_for_test`
のような追加関数が不要)。

### 3.3 公開 API

```rust
impl SettingsDb {
    pub fn open(data_dir: &Path) -> Result<Self, SettingsDbError>;

    /// 全テーブルを読んで Settings を再構築する。
    /// 完了時点で in-memory と DB が一致するので、VST3 hash を初期化する。
    pub fn load_into_settings(&self) -> Result<Settings, SettingsDbError>;

    /// 純粋な永続化。rotation には触れない (= bootstrap save と user save 両方が共有)。
    /// - 小サイズ table: transaction 内で DELETE+INSERT (削除・並べ替えを反映)
    /// - VST3 大型 table: hash で変更検出、未変更なら skip
    /// - commit 成功後にのみ hash 更新
    pub fn save_full(&self, settings: &Settings) -> Result<(), SettingsDbError>;

    /// hot-path 用、変更のあった row だけを upsert する。
    /// (Phase 0 の実測結果次第で必要性を判定)
    pub fn upsert_video_resume_position(&self, path: &str, secs: f64) -> Result<(), SettingsDbError>;
    pub fn remove_video_resume_position(&self, path: &str) -> Result<(), SettingsDbError>;

    /// VACUUM INTO で snapshot を作成。target が存在しないことを呼び出し側で保証する。
    pub fn backup_to(&self, target: &Path) -> Result<(), SettingsDbError>;
}
```

### 3.4 `Settings::load()` / `Settings::save()` の差し替え

```rust
impl Settings {
    pub fn load() -> Self {
        match with_db_result(|db| db.load_into_settings()) {
            Ok(s) => s,
            Err(e) => {
                settings_diag_log(&format!("settings: load failed: {e:?}"));
                MAIN_UNREADABLE_THIS_SESSION.store(true, Ordering::Relaxed);
                Self::default()
            }
        }
    }

    /// user save。プロセス初回呼び出しで rotation を走らせる。
    /// 失敗時は log のみ、in-memory state は維持。
    #[track_caller]
    pub fn save(&self) {
        // Phase 0 計装 (env gate、実測後削除)
        if std::env::var_os("MIV_SETTINGS_SAVE_TRACE").is_some() {
            let caller = std::panic::Location::caller();
            settings_diag_log(&format!(
                "settings: save called from {}:{}",
                caller.file(), caller.line()
            ));
        }
        if MAIN_UNREADABLE_THIS_SESSION.load(Ordering::Relaxed) {
            return;
        }
        let result = with_db_result(|db| {
            // 初回のみ rotate (現行の `BACKUP_DONE_THIS_SESSION` ポリシーを踏襲)
            if !BACKUP_DONE_THIS_SESSION.swap(true, Ordering::Relaxed) {
                rotate_db_backups(db, &Settings::db_path())?;
            }
            db.save_full(self)
        });
        if let Err(e) = result {
            settings_diag_log(&format!("settings: save failed: {e:?}"));
        }
    }
}
```

## 4. スキーマ

実型は [src/settings.rs](../src/settings.rs) を正とする。schema は実型に追従する責任を負う
(`#[derive(serde::*)]` の field を追加・削除したら schema も更新)。

```sql
-- メタ情報
CREATE TABLE schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- ('schema_version', '1'), ('migrated_from_json_at', '<unix_ts>'), ('app_version', '0.9.0')
-- app_version は DB open では更新せず、save_full の commit 内で「最後に正常保存した版」を記録する。

-- スカラ設定 (約 80 個のフィールド)
CREATE TABLE settings_kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL  -- JSON encoded (bool / int / string / 小 array)
);

-- favorites (現状 14 件、UUID キー、Vec 順序つき)
CREATE TABLE favorites (
    id                    BLOB PRIMARY KEY,   -- Uuid (16 bytes)
    name                  TEXT NOT NULL,
    path                  TEXT NOT NULL,
    sort_index            INTEGER NOT NULL,
    auto_index_structure  INTEGER NOT NULL DEFAULT 0,
    auto_index_metadata   INTEGER NOT NULL DEFAULT 0,
    auto_index_thumbs     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX favorites_sort ON favorites(sort_index);

-- tags (TagDef は id + name のみ、Vec 順序保持のため sort_index 追加)
CREATE TABLE tags (
    id          BLOB PRIMARY KEY,
    name        TEXT NOT NULL,
    sort_index  INTEGER NOT NULL
);
CREATE INDEX tags_sort ON tags(sort_index);

-- video resume positions (現状 176 件、hot-path)
CREATE TABLE video_resume_positions (
    path_normalized TEXT PRIMARY KEY,
    position_secs   REAL NOT NULL,
    updated_at      INTEGER NOT NULL    -- unix epoch、将来の auto-prune 用
);

-- VST3 chain (現在ロード中、3.7MB BLOB の主)
-- 同一 chain 内で plugin_path は一意 (preferences UI で重複弾いている)
CREATE TABLE vst3_plugins (
    plugin_path  TEXT PRIMARY KEY,
    chain_index  INTEGER NOT NULL UNIQUE,    -- 0..MAX_CHAIN_LEN-1
    plugin_name  TEXT,
    bypass       INTEGER NOT NULL DEFAULT 0,
    user_hidden  INTEGER NOT NULL DEFAULT 0,
    gui_pos_x    INTEGER, gui_pos_y INTEGER,
    gui_size_w   INTEGER, gui_size_h INTEGER,
    state        TEXT                       -- Option<String> (base64) をそのまま格納
);

-- VST3 chain slots (preset 10 個、3.5MB BLOB)
-- plugins は Vec<Vst3PluginEntry> を JSON で持つ (BLOB を含むので分離は複雑、JSON で OK)
CREATE TABLE vst3_chain_slots (
    slot_index     INTEGER PRIMARY KEY,    -- 0..9
    name           TEXT NOT NULL,
    gui_visible    INTEGER NOT NULL DEFAULT 1,
    video_compact  INTEGER NOT NULL DEFAULT 0,
    plugins_json   TEXT NOT NULL
);

-- recent / custom apps (Vec 順序保持)
CREATE TABLE recent_open_with_apps (
    exe_path      TEXT PRIMARY KEY,
    display_name  TEXT NOT NULL,
    sort_index    INTEGER NOT NULL,
    launch_kind   TEXT                 -- Executable / Association / OsDefault
);
CREATE INDEX recent_apps_sort ON recent_open_with_apps(sort_index);

CREATE TABLE custom_open_with_apps (
    exe_path      TEXT PRIMARY KEY,
    display_name  TEXT NOT NULL,
    sort_index    INTEGER NOT NULL
);
CREATE INDEX custom_apps_sort ON custom_open_with_apps(sort_index);

-- external tools (stable id + Vec order; same executable may be registered repeatedly)
CREATE TABLE external_tools (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT NOT NULL,
    launch_kind          TEXT NOT NULL, -- Executable / Association / OsDefault
    launch_value         TEXT,          -- EXE path / handler ID / NULL
    arguments            TEXT NOT NULL,
    working_directory    TEXT,
    payload              TEXT NOT NULL,
    video                TEXT NOT NULL,
    spread               TEXT NOT NULL,
    selection            TEXT NOT NULL,
    pdf_render_long_edge INTEGER NOT NULL,
    for_editing          INTEGER NOT NULL,
    show_in_context_menu INTEGER NOT NULL,
    keep_temp            INTEGER NOT NULL,
    sort_index           INTEGER NOT NULL
);
CREATE INDEX external_tools_sort ON external_tools(sort_index);
```

`custom_open_with_apps` はリリース済みの移行元として行を残し、mIV からは以後更新しない。
`schema_meta.external_tools_migrated_from_custom_open_with = '1'` が無い場合だけ、
`sort_index` 順に `external_tools` へ変換し、行と marker を同じ transaction で commit する。
marker が失われても `external_tools` に行があれば追加移行せず、二重登録を防ぐ。

`recent_open_with_apps.exe_path` はリリース済み列名なので、P1c 後も起動値の carrier として残す。
既存テーブルへ nullable `launch_kind` を追加し、
`schema_meta.recent_open_with_apps_launch_classified = '1'` が無い場合だけ、未分類
(`launch_kind IS NULL`) の各行を分類する。実在するファイルなら `Executable`、それ以外は
`Association` とし、更新と marker を同じ transaction で commit する。marker が失われても
非 NULL の行は再分類しないため、後から同名パスが作られても起動種別は変わらない。

`external_tools` は P1c 時点で未出荷だったため、旧 `executable` schema からのデータ移行は行わず
テーブルを新 schema へ作り直す。released source の `custom_open_with_apps` は残っているため、
既存の一度きり移行を再実行して `Executable` として登録し直す。

### 4.1 PRAGMA 設定 (open 時)

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA wal_autocheckpoint = 1000;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;
```

### 4.2 save_full の擬似コード

```rust
pub fn save_full(&self, s: &Settings) -> Result<(), SettingsDbError> {
    let mut inner = self.inner.lock().map_err(|_| SettingsDbError::Poisoned)?;

    // 1. 事前 hash 計算 (transaction 外)
    let chain_hash = hash_vst3_plugins(&s.vst3_plugins);
    let slots_hash = hash_vst3_chain_slots(&s.vst3_chain_slots);
    let chain_changed = inner.last_saved_vst3_chain_hash.map_or(true, |h| h != chain_hash);
    let slots_changed = inner.last_saved_vst3_slots_hash.map_or(true, |h| h != slots_hash);

    // 2. transaction 内で DB 更新
    let tx = inner.conn.unchecked_transaction()?;
    upsert_kv_scalars(&tx, s)?;
    delete_and_insert_favorites(&tx, &s.favorites)?;
    delete_and_insert_tags(&tx, &s.tags)?;
    delete_and_insert_video_resume_positions(&tx, &s.video_resume_positions)?;
    delete_and_insert_recent_apps(&tx, &s.recent_open_with_apps)?;
    // custom_open_with_apps は読み取り専用の移行元として既存行を残し、書き戻さない
    delete_and_insert_external_tools(&tx, &s.external_tools)?;
    if chain_changed {
        delete_and_insert_vst3_plugins(&tx, &s.vst3_plugins)?;
    }
    if slots_changed {
        delete_and_insert_vst3_chain_slots(&tx, &s.vst3_chain_slots)?;
    }
    tx.commit()?;  // 成否の境界

    // 3. commit 成功してから hash 更新
    if chain_changed { inner.last_saved_vst3_chain_hash = Some(chain_hash); }
    if slots_changed { inner.last_saved_vst3_slots_hash = Some(slots_hash); }
    Ok(())
}
```

hash 関数は `xxhash-rust` か `std::collections::hash_map::DefaultHasher`。衝突しても次回の
完全 save で復旧するので cryptographic 強度不要。

### 4.3 load 時の hash 初期化

```rust
pub fn load_into_settings(&self) -> Result<Settings, SettingsDbError> {
    let mut inner = self.inner.lock().map_err(|_| SettingsDbError::Poisoned)?;
    let settings = build_settings_from_db(&inner.conn)?;

    // load 完了時点で DB と in-memory が一致しているので hash を同期しておく。
    // これがないと起動後最初の save_full で VST3 row を無駄に DELETE+INSERT する。
    inner.last_saved_vst3_chain_hash = Some(hash_vst3_plugins(&settings.vst3_plugins));
    inner.last_saved_vst3_slots_hash = Some(hash_vst3_chain_slots(&settings.vst3_chain_slots));

    Ok(settings)
}
```

## 5. 起動時の決定木

T06 (v0.9.0) で bool 判定から tri-state (`FamilyPresence::{Present, ConfirmedAbsent,
Ambiguous}`) + brief retry に置き換えた。AV / cloud sync の一瞬の I/O blip で
「ファイル無し」と誤判定して clean install に倒れる事故 (= 旧データを defaults で
上書き) を防ぐ。

```
SettingsDb::open() の判定フロー
─────────────────────────────────────────────────────
settings_db_family_presence_with_retry(data_dir) で family を tri-state 判定
※ family = settings.db, settings.db-wal, settings.db-shm, settings.db.bak1..bak10
※ 判定は per-file metadata + read_dir の二経路で robust 化
※ Ambiguous は 3 回試行 + 80ms backoff で stabilize を試みる

├─ Present (= family が見える)
│   → SettingsDb::open(settings.db) を試行
│   ├─ schema_meta.app_version > 現バイナリ
│   │   → PRAGMA / schema 更新前に IncompatibleSettings + save 抑止
│   │   → 設定復元または終了を選ぶ案内を表示 (初回設定・更新案内は表示しない)
│   ├─ Success → integrity_check OK?
│   │       ├─ YES → 通常 load
│   │       │   ├─ deserialize 成功 → hash 同期 → 完了
│   │       │   └─ unknown variant / unknown field
│   │       │       → IncompatibleSettings + save 抑止
│   │       │       → main / WAL / SHM / bak1..bak10 は一切変更しない
│   │       └─ NO (PRAGMA integrity_check が "ok" 以外) → Corrupted 扱い (下記)
│   ├─ Transient failure
│   │   (DatabaseBusy / DatabaseLocked / CannotOpen / SystemIoFailure)
│   │   → 50ms backoff で最大 3 回 retry
│   │   → それでも失敗 → FailedFallbackDefault + save 抑止
│   │   → 設定復元または終了を選ぶ保護モーダルを表示
│   │   ※ settings.json への fallback は **絶対にしない** (新 DB 変更巻き戻し防止)
│   └─ Corrupted (NotADatabase / DatabaseCorrupt / integrity_check 失敗 /
│                 bootstrap_complete marker 欠落)
│       → quarantine: settings.db, .db-wal, .db-shm を 3 セットで .corrupted-<ts> リネーム
│       → settings.db.bak1, bak2, ... を新しい順に試行
│       → 全滅 → FailedFallbackDefault + save 抑止
│
├─ Ambiguous (= metadata の non-NotFound エラーや read_dir 失敗が続いた)
│   → FailedFallbackDefault + save 抑止 で immediate 返却
│   → 設定復元または終了を選ぶ保護モーダルを表示
│   ※ defaults で書き込まないので、状況が改善した次回起動で正しく family を発見できる
│
└─ ConfirmedAbsent (= 全 metadata NotFound + (read_dir 成功 / data_dir NotFound))
    → legacy_json_family_presence_with_retry(data_dir) を判定
    ├─ Present (= settings.json が存在、旧バージョンからの初回起動)
    │   → JSON migration:
    │   1. try_load_with_recovery(settings.json) で読む
    │   2. 新規 settings.db を作成 + schema 適用 (= create_new)
    │   3. SettingsDb::save_full(&loaded) で bootstrap_complete マーカーと共に永続化
    │      ※ save_full 失敗時は drop(db) + cleanup_orphan_db_after_failed_bootstrap で
    │         settings.db / -wal / -shm を削除して FailedFallbackDefault に倒す
    │         (T06: orphan を残すと次回起動の family=Present 判定が quarantine 経路に倒れる)
    │   4. settings.json → settings.json.migrated-<ts>
    │   5. settings.json.bak1..bak10 → settings.json.bakN.migrated-<ts>
    │
    ├─ Ambiguous → FailedFallbackDefault + save 抑止 (= JSON を破棄しない)
    │
    └─ ConfirmedAbsent (= 旧 JSON も無い)
        → clean install:
        1. 新規 settings.db を作成 + schema 適用
        2. SettingsDb::save_full(&Settings::default()) で初期化
           ※ 失敗時は migration 経路と同じく orphan cleanup + FailedFallbackDefault
        3. bak rotation は最初の user save まで遅延
```

### 5.1 SQLite エラー分類

```rust
fn classify_open_error(e: &rusqlite::Error) -> OpenFailureKind {
    let err = match e {
        rusqlite::Error::SqliteFailure(err, _) => err,
        _ => return OpenFailureKind::Other,
    };
    // 診断ログには extended_code も残す (SystemIoFailure 内訳の特定用)
    settings_diag_log(&format!(
        "settings: db open error: primary={:?} extended={}",
        err.code, err.extended_code
    ));
    use rusqlite::ErrorCode::*;
    match err.code {
        NotADatabase | DatabaseCorrupt => OpenFailureKind::Corrupted,
        DatabaseBusy | DatabaseLocked | CannotOpen | SystemIoFailure
            => OpenFailureKind::Transient,
        PermissionDenied | ReadOnly => OpenFailureKind::Permission,
        _ => OpenFailureKind::Transient,
    }
}
```

quarantine は **Corrupted のみ** (NotADatabase / DatabaseCorrupt / integrity_check 失敗)。
SystemIoFailure 等の I/O 系は transient 扱いで save 抑止のみ、ファイル移動はしない。
`Settings` の deserialize 中に `unknown variant` / `unknown field` が出た場合は、物理破損では
なく「新しい版で保存した設定を古い版が読んだ」可能性が高いため `Incompatible` とする。
この場合も save 抑止のみで、main やバックアップを quarantine・復旧コピーしてはならない。
その他の型不一致（数値フィールドに文字列など）は従来どおり `Corrupted` とする。

## 6. バックアップ戦略

### 6.1 世代モデル (現行踏襲)

`settings.db.bak1` 〜 `settings.db.bak10` の 10 世代。**プロセスの最初の user save** で 1 回だけ
rotation。bootstrap (clean install / JSON migration) では rotation を走らせない。
各 DB の `schema_meta.app_version` は、その内容を最後に正常保存した mImageViewer の版を示す。
open だけでは更新しないため、rotation 前の版が bak 側に保持される。

```
rotate_db_backups:
1. settings.db.bak10 を削除
2. settings.db.bak9 → settings.db.bak10 (rename)
3. settings.db.bak8 → settings.db.bak9
...
9. settings.db.bak1 → settings.db.bak2
10. SettingsDb::backup_to('settings.db.bak1')
    = VACUUM INTO 'settings.db.bak1'
    - 制約: target は存在してはいけない (rename で空けた後)
    - 制約: VACUUM INTO は transaction の外で実行する
```

VACUUM INTO は SQLite が**新規 .db ファイルに consistent な snapshot を作る** primitive。
元 DB を rename しないので、現行 JSON の rotation race は構造的に消える。

### 6.2 .db-wal / .db-shm の扱い

- backup 対象には含めない (VACUUM INTO は wal/shm の内容を統合して 1 つの .db に書き出す)
- quarantine は **all-or-nothing rollback** で 3 つセットを atomic にリネームする
  (T06 v0.9.0):
  ```
  順序: -wal → -shm → main
  suffix: <ts>-<seq>  (秒精度 + プロセス内 atomic カウンタで衝突回避)

  settings.db-wal  → settings.db-wal.corrupted-<ts>-<seq>
  settings.db-shm  → settings.db-shm.corrupted-<ts>-<seq>
  settings.db      → settings.db.corrupted-<ts>-<seq>
  ```
  途中で 1 つでも rename が失敗すると、**既に成功していた rename を逆順で undo** して
  家族を元の状態に戻し、`QuarantineResult { moved: 0, complete: false }` を返す。
  これで「-wal だけ動いて main が残った」partial-move 状態を disk に残さず、次回起動で
  SQLite が main を WAL 不在で開き stale checkpoint を silently load する事故を防ぐ。

  rollback 自体が失敗した稀な状況 (= 永続的な I/O 障害) では `PERSISTENT-DISK-INCONSISTENCY`
  ログを出し、`complete: false` を返す。caller (`boot_recover_from_bak`) は bak 復旧経路に
  進まず即 `FailedFallbackDefault` で save 抑止に倒れる。ユーザーが手動で `.corrupted-*` を
  整理すれば次回起動で正常復旧可能。

### 6.3 復旧失敗時の挙動

`bak1..bak10` を順に試して全滅した場合、または quarantine が `complete: false` で返った場合:
- `set_save_suppressed(true)` を立てる
- `Settings::default()` を返す
- セッション中の `Settings::save()` は全て suppress (= disk 上の残骸を保護)
- 次回起動時に手動復旧 (= `.corrupted-<ts>-<seq>` の調査) ができる状態を維持
- 「設定の読み込みに失敗しました」モーダルを表示し、設定復元または終了を選ばせる。
  初回設定・重要な変更点・操作カスタマイズ取込は出さず、既定設定を通常起動と誤認させない

### 6.4 設定の復元 UI とダウングレード

「設定の復元」は main / bak1..bak10 に加えて `settings.db.preupgrade-v<old>` も一覧化し、
各行に保存元版と現バイナリとの版互換性を表示する。通常世代は `schema_meta.app_version`、
preupgrade は旧実装で metadata が open 時に上書きされていた可能性があるためファイル名の
`v<old>` を正とする。現バイナリより新しい版の行は復元不可とし、同版・旧版・版不明の候補も、実際の置換前に一時コピーを
`SettingsDb::open + load_into_settings` して最終検証する。

互換性ガードを備えた版で、それより新しい版が保存した設定を起動した場合は main / WAL /
SHM / backup chain を変更せず、セッション全体の設定保存を抑止する。専用モーダルから
「設定の復元」またはアプリ終了を選び、復元画面を閉じた場合は保護モーダルへ戻る。
「このまま使う」は提供しない。設定保存だけを抑止しても、旧バイナリによる他の永続DBや
ファイル操作までは読み取り専用にならないためで、設定を保存した版以降の起動を第一案内とする。
復元または全リセットがファイル差し替え前に失敗した場合、save 抑止は操作開始時の状態へ戻す。
通常起動からの失敗では操作を継続できる一方、非互換・読み込み失敗によるブート保護中は
抑止を維持し、既定設定による unreadable な DB の上書きを許可しない。
このガードは v2.6.0 からのため、既公開の v2.5.0 以前へ戻す場合は下記 10.5 の
保存形式互換も併用する。

### 6.5 失敗 bootstrap の orphan cleanup

`SettingsDb::create_new` または `save_full` が **bootstrap_complete マーカーを書く前**に
失敗すると、SQLite が `Connection::open_with_flags(SQLITE_OPEN_CREATE)` で作った
`settings.db` (+ `-wal` / `-shm`) が **marker 無し orphan** としてディスクに残る。
次回起動で family が見えてしまい quarantine 経由でしか migration を再試行できなくなる
(旧 JSON は無傷だが UX 劣化)。

`cleanup_orphan_db_after_failed_bootstrap(data_dir)` がこれを掃除する (T06 v0.9.0):
- 削除前に `probe_bootstrap_marker(data_dir)` で **marker の確実な欠落** を確認
  - `Present` → 削除しない (= pre-check が見落とした実 DB を保護)
  - `ConfirmedAbsent` → 削除する
  - `Unknown` (file 不在 / open 失敗 / query エラー) → 削除しない (= safe-by-default)
- 削除順序は sidecar (`-wal` / `-shm`) → main。sidecar 削除に失敗したら main を残し、
  次回起動の classifier が改めて Corrupted 判定して quarantine 経路に倒せる
  (= 「main 無し + sidecar 残り」のロック状態を作らない)
- 呼び出し箇所: `SettingsDb::create_new` の open 失敗 (ただし `AlreadyBootstrapped` は除外)、
  `migrate_from_settings_json` の save_full 失敗、`boot_settings_db_inner` clean install の
  save_full 失敗

## 7. 移行された JSON ファイルの扱い

migration 完了時にリネームする:

```
settings.json         → settings.json.migrated-<ts>
settings.json.bak1    → settings.json.bak1.migrated-<ts>
settings.json.bak2    → settings.json.bak2.migrated-<ts>
...
settings.json.bak10   → settings.json.bak10.migrated-<ts>
```

**リネーム派の理由**: 旧バージョンへの downgrade 時、`settings.json` が存在しないので「真の初回起動」
扱いで Default 起動になる。これは「気付かないうちに古い bak が main に昇格」より安全。手動 downgrade
したい上級ユーザーは `.migrated-<ts>` を `settings.json` に戻せば従来の bak チェーンも復活する。

ドキュメント (本書 + 必要なら `docs/migration-notes.md`) に手動 downgrade 手順を明記。

## 8. 並列 `Settings::load()` の撲滅

現状 `Settings::load()` は以下 4 箇所から呼ばれる:

| 場所 | スレッド | Phase 4 での対応 |
|---|---|---|
| [main.rs:612](../src/main.rs) | main | そのまま (起動時 1 回) |
| [app.rs:2590](../src/app.rs) `App::default()` | main | **削除**、main の `saved` を `App::new(saved)` で受け取る |
| [folder_tree.rs:330](../src/folder_tree.rs) `sorted_subdirs` | UI 周辺 + DFS バックグラウンド | **削除**、`skip_zip_if_folder_exists` と `sort_order` を引数で受ける |
| [susie_loader.rs:441](../src/susie_loader.rs) `get_pool` | susie-init thread | **削除**、`init_pool(enabled, parallel)` を追加し main から呼ぶ |

これで `Settings::load()` は **プロセスにつき 1 回、main thread 上でのみ** 呼ばれる不変条件にする。

### 8.1 folder_tree の波及

`sorted_subdirs` の signature 変更は `next_folder_dfs` / `prev_folder_dfs` / その他 DFS 呼び出し点に
波及する。Phase 4 で全部一括変更。

### 8.2 susie_loader の互換維持

`get_pool() -> Arc<...>` は無引数のまま維持し、新たに `init_pool(enabled: bool, parallel: bool)` を
追加。`main.rs` の起動シーケンスで `init_pool` を先に呼んでから、それ以降は `get_pool` が初期化済み
プールを返す。

## 9. 実装フェーズ

各 phase は独立コミット (1 PR にまとめる前提でも分離可能)。各 phase 完了時点で Codex に
コマンドラインで review 依頼 → 指摘対応 → 次 phase。

### Phase 0: 現状調査 (~30 分)

**目的**: 現在のコードで `Settings::save()` がどこから何回呼ばれているかを実測し、Phase 3 (hot path
最適化) の必要性を確定する。

実装:
- `Settings::save()` に `#[track_caller]` + env-gated `settings_diag_log` を追加
- env `MIV_SETTINGS_SAVE_TRACE=1` で有効化、通常時は no-op

実測手順:
1. パッチ投入したビルドで `MIV_SETTINGS_SAVE_TRACE=1 cargo run --release` (or 同等)
2. 通常使用 30 分 (起動 → フォルダ操作 → 動画再生 → 設定変更 → on_exit)
3. `settings.log` から save callsite と頻度を集計
4. 結果を docs/ にメモ、Phase 3 のスコープを確定

Phase 0 のパッチコードは Phase 6 (実装後) で削除する。

### Phase 1: SettingsDb infrastructure (~1.5 日)

- `src/settings_db.rs` 新規
- スキーマ定義 + `init_schema()`
- `SettingsDb { inner: Mutex<Inner> }` 構造
- `open()` (`PRAGMA` 設定 + integrity_check + エラー分類 + retry)
- `load_into_settings()` (hash 初期化付き)
- `save_full()` (transaction + DELETE+INSERT + hash skip + commit 成功時 hash 更新)
- `backup_to()` (VACUUM INTO ラッパー)
- `with_db` / `with_db_result` の lazy global
- `settings_db_family_presence()` / `settings_db_family_presence_with_retry()`
  (T06 v0.9.0 で bool ベースの `*_exists()` を tri-state に置き換え)
- unit tests: in-memory SQLite でラウンドトリップ、hash skip 動作、エラー分類

### Phase 2: JSON migration (~0.5 日)

- `migrate_from_settings_json(path)` 実装
- 現行の `try_load_with_recovery` を流用 (bak1..bak10 を遡る現行ロジックそのまま)
- 成功後の `.migrated-<ts>` リネーム
- integration test: tempdir に旧 settings.json (実データ複製) を置いて、起動後 settings.db に同等内容が
  入ることを確認

### Phase 3: `Settings::load()` / `Settings::save()` 差し替え (~1 日)

- 旧 load/save の内部実装を SettingsDb 経由に置換
- `MAIN_UNREADABLE_THIS_SESSION` / `BACKUP_DONE_THIS_SESSION` 等のフラグは保持 (semantics 維持)
- 旧 `try_load_with_recovery` / `rotate_backups` / `write_atomic` save は **削除しない**
  (`#[deprecated]` で deprecated 化、数バージョン後に削除)
- 既存 unit test の `setup_backup_env()` を SQLite tempdir 対応に書き換え
- Phase 0 の結果次第で hot-path `upsert_*` 経由への切替

### Phase 4: 並列 load 撲滅 (~0.5 日)

- `folder_tree::sorted_subdirs` の signature 変更 (skip_zip + sort_order を引数化)
- `folder_tree::next_folder_dfs` / `prev_folder_dfs` も同様
- `susie_loader::init_pool(enabled, parallel)` を追加、`main.rs` から呼ぶ
- `susie_loader::get_pool()` は無引数のまま、初期化済みプールを返すように変更
- `app.rs:2590` の `Settings::load()` を削除、main からの引数で受ける

### Phase 5: バックアップ移行 (~0.5 日)

- `rotate_db_backups()` 実装 (rename → VACUUM INTO bak1)
- 起動マイグレーション後の `.migrated-<ts>` リネーム処理
- 旧 `*.json.bak*` ファイルは放置 (= migration 時に `.migrated-<ts>` 化済みなので干渉しない)

### Phase 6: Phase 0 計装の削除 + クリーンアップ (~0.5 日)

- `#[track_caller]` + env gate log を削除
- 旧 `*.json` save 経路 (`#[deprecated]` 化済) を削除 (deprecation 期間後)
- ドキュメント整合性チェック

### Phase 7: テスト + ドキュメント (~1 日)

- `setup_backup_env()` の SettingsDb 版を新規追加
- 既存 `app/tests.rs` の `phase_c_support` を SQLite-aware に
- 本ドキュメントを最新化 (実装で得られた知見を反映)
- [CLAUDE.md](../CLAUDE.md) の「世代バックアップ」記述を SQLite 版に書き換え
- [docs/architecture-overview.md](architecture-overview.md) のモジュールマップに `settings_db.rs` 追加
- [README.md](../README.md) 更新履歴に 1 行

### 工数

| Phase | 工数 |
|---|---|
| 0 | 0.5 日 |
| 1 | 1.5 日 |
| 2 | 0.5 日 |
| 3 | 1 日 |
| 4 | 0.5 日 |
| 5 | 0.5 日 |
| 6 | 0.5 日 |
| 7 | 1 日 |
| **合計** | **6 日** |

## 10. 既知のリスク / 既知の論点

### 10.1 SQLite open 自体が transient で失敗するケース

`Connection::open()` も内部で `CreateFileW` を呼ぶ。AV が settings.db を一瞬隠したら open 失敗。
対策は決定木 (§5) どおり: retry → 失敗時 save 抑止。**handle が開いた後は再現しない** ので、起動時の
1 回のリスクのみ。

### 10.2 hash 衝突

`DefaultHasher` (FxHash 系) の衝突確率は 64bit で実用上ゼロ。仮に衝突しても、次回の VST3 内容変更で
hash が変わって save される。実害は「1 回だけ古い state を残す」だけで、データ損失ではない。

### 10.3 設定の手動編集が消える

`notepad settings.json` workflow ができなくなる。代替:
- `sqlite3 settings.db "UPDATE settings_kv SET value='...' WHERE key='...';"` (geek 向け)
- 環境設定画面は既存実装で多くを網羅
- 将来: 環境設定に「設定を JSON にエクスポート / 取り込み」を追加 (optional)

### 10.4 既存 SQLite DB との connection 統一は当面しない

mImageViewer は既に catalog / rating / rotation / audio_normalize / video_pins 等で SQLite を多用
しているが、それぞれが独立 Connection を持っている。SettingsDb も同様に独立で開く。共通の
PRAGMA / busy_timeout / retry helper を `src/db_common.rs` に切り出すのは optional な
クリーンアップ (Phase 7+ で検討)。

### 10.5 旧バージョンとの互換

`.migrated-<ts>` と `settings.db.preupgrade-v<old>` を残す。v2.6.0 以降のバイナリは、
自分より新しい版が保存した DB を `schema_meta.app_version` で検出し、設定を上書きせず、
復元画面で保存元バージョンを確認して互換候補へ戻せる。

既公開の v2.5.0 以前にはこの起動前ガードを後付けできない。そのため v2.6.0 の保存では、
既存フィールドへ追加した未知 enum (`FacetDatePreset` の追加日付候補、詳細表示のページ数
ソート・列・列幅) を、旧版が無視できる追加フィールドへ退避してから書く。リング／ジェスチャ
の新アクションは、旧版側にも未知文字列の受け皿があるため設定全体の読み込み失敗にはならない。
これにより v2.5.0 へ戻しても設定 DB 全体は読み込めるが、旧版が v2.6.0 専用フィールドを
保存し直すと、その専用設定（スマートフォルダなど）は保持されない。旧版での利用後に
v2.6.0 へ戻す場合は、必要に応じて設定の復元から v2.6.0 保存世代を選ぶ。

## 11. 並行して入れる緊急パッチ (SQLite 化と独立、優先度 高)

SQLite 化が完了するまでの暫定対策。SQLite 化後は不要だが、`write_atomic` は他の用途
(`extract_embedded_file` 等) で使われ続けるので、その分は移行後も価値が残る。

### 11.1 `write_atomic` の `remove_file` を削除 — 数行

[data_dir.rs:116-122](../src/data_dir.rs):

```rust
// BEFORE
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp_path = tmp_path_for(path);
    std::fs::write(&tmp_path, bytes)?;
    let _ = std::fs::remove_file(path);   // ← 削除
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

// AFTER
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp_path = tmp_path_for(path);
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, path)?;  // MOVEFILE_REPLACE_EXISTING でアトミック
    Ok(())
}
```

Rust の `std::fs::rename` は Windows でも `MoveFileEx(MOVEFILE_REPLACE_EXISTING)` を使うので、
既存ファイル上書きが OS レベルでアトミックに行われる。`remove_file` で作っていた race window が消える。

### 11.2 Default fallback での save 抑止 — 数十行

[settings.rs:1902-1922](../src/settings.rs) の `unwrap_or_else` 経路で、synthetic Default に落ちたら
**真の初回起動を `read_dir` ベースで robust 判定**し、それ以外なら `MAIN_UNREADABLE_THIS_SESSION` を
立てて session-wide で save 抑止する。

```rust
let mut settings = outcome.settings.unwrap_or_else(|| {
    settings_diag_log("settings: no readable settings/backup found; using built-in default");
    // 真の初回起動判定: per-file metadata 系列ではなく read_dir で親 dir を列挙
    let truly_first_launch = !any_settings_files_via_readdir(&path);
    if !truly_first_launch {
        MAIN_UNREADABLE_THIS_SESSION.store(true, Ordering::Relaxed);
        settings_diag_log(
            "settings: synthetic default in non-first-launch context; \
             suppressing all save() this session"
        );
    } else {
        settings_diag_log(
            "settings: synthetic default in truly-first-launch context; save() allowed"
        );
    }
    Self::default()
});
```

`any_settings_files_via_readdir` は `std::fs::read_dir(parent)` で `settings.json*` を列挙する関数
(per-file `metadata` と別系統の syscall で transient NotFound を回避)。3 回 retry + 1 つでも
見えたら true。

これにより:
- 真の初回起動: read_dir で何も見えない → save 許可 → 初期 settings.json 作成
- transient I/O 失敗: read_dir で残骸が見える → save 抑止 → 現状の disk state を維持

### 11.3 診断ログに `raw_os_error()` を追加 — 数行

[settings.rs:1701-1730](../src/settings.rs) の `log_one_file_snapshot` で `.kind()` だけでなく
`.raw_os_error()` も log する。Windows の 6 種類の NotFound 系エラーを区別可能になる:
`ERROR_FILE_NOT_FOUND (2)` / `ERROR_PATH_NOT_FOUND (3)` / `ERROR_INVALID_DRIVE (15)` /
`ERROR_BAD_NETPATH (53)` / `ERROR_BAD_NET_NAME (67)` / `ERROR_NOT_FOUND (1168)` のどれか。

SQLite 化後は不要だが、移行期間中の保険として有用。

## 12. ワークフロー

1. **Claude が本 spec doc を最終版として確定** (= このファイル)
2. **Claude が Phase 0 (計装) 実装** → ユーザに 30 分使用してもらう → `settings.log` を解析 → Phase 3 のスコープ確定
3. **Phase 1 から Claude が順次実装**、各 phase 完了ごとに `codex exec --sandbox read-only -o /tmp/codex-review-phaseN.txt "..."` でレビュー要求 → 指摘対応 → 次 phase
4. **全 phase 完了後**、Claude が実機動作確認できる範囲のスモークテスト
5. **ユーザが Codex GUI から全体レビューを再依頼** → 修正があれば Claude が反映

## Appendix A: Codex review 履歴

本 spec は 2026-05-13 の作業で Codex に 4 回 review を依頼し、その指摘を反映したもの。主な
合意事項:

### Round 1 の合意

- **Phase 6 (hot path) は Phase 3 に統合**: save_full の VST3 hash skip で根本対応するため
- **Connection ownership = lazy global (案 C)**: `data_dir::set_test_override` 連動で
  test override 時は自動 re-open
- **DB open failure の分類**: NotADatabase / DatabaseCorrupt / integrity_check 失敗のみ
  quarantine、その他は transient 扱い
- **WAL/SHM を 3 セットで quarantine**: 古い wal が新 DB の recovery で誤読されるのを防ぐ
- **backup ordering**: rotate (VACUUM INTO bak1) → save の順序を明文化

### Round 2 の合意

- **5 秒ごとの save は現コードで既に消えている可能性**: Phase 0 で実測してから Phase 3 のスコープ確定
- **`with_db` は `Result` を返す、`expect` は使わない**: transient 失敗時に panic させない
- **VST3 重複 path は弾く**: `plugin_path PRIMARY KEY` + `chain_index UNIQUE`
- **`tags` / `RecentApp` のスキーマを実型に合わせる**: tags=(id, name, sort_index)、
  RecentApp=(exe_path, display_name, sort_index)
- **SQLite エラーの extended_code もログ**: SystemIoFailure の内訳を特定可能に

### Round 3 の合意

- **VST3 大型 row は hash で dirty 検出、Phase 1 必須**: save_full の hot path skip
- **commit 成功後にのみ hash 更新**: 「メモリ更新済み、DB 未更新」状態を防ぐ
- **Vec table は DELETE+INSERT**: upsert だけだと削除・並べ替えが反映されない
- **VST3 state は当面 TEXT のまま** (Option<String> = base64)
- **`#[track_caller]` で計装**: backtrace 取得より軽量
- **DB 初回作成 / migration / fallback 決定木の明文化** (本書 §5)

### Round 4 の合意

- **`save_full` (pure) と `Settings::save()` wrapper (rotation 含む) を分離**: bootstrap で
  意図せず rotate しないように
- **load 完了時に VST3 hash 初期化**: 初回 save_full での無駄な DELETE+INSERT 防止
- **`settings.db` family 存在判定**: sidecar / bak も含めて見る (transient で main 1 ファイルが
  見えないだけで「DB なし」と誤判定しない)
- **`with_db_result` helper**: nested Result の握り潰し防止
- **`Mutex<Inner>` 統合**: hash と connection のロック順問題を排除
- **track_caller log は env/perf gate**: 計装が測定対象に I/O を足さない
- **`.migrated-*` リネーム**: 旧 bak も同時に rename して downgrade を安全に
