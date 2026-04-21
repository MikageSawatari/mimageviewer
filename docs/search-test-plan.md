# 検索機能テスト整備計画 (v0.8.0)

Ctrl+G グローバルメタ検索、名前索引、notify-rs FS 監視、Ctrl+↑↓ ナビゲーションを
自動テストでカバーするための計画と進捗。

## 背景

v0.8.0 で検索・インデクサ機能が大きく拡張されたが、統合テスト層が薄い:

- `search_watcher` は `absorb_event` の単体テストのみ (実 FS 経路未カバー)
- 名前索引は (修正前) ワンショットなので watcher すらない
- Ctrl+G / Ctrl+↑↓ は `App` メソッドで、`App` は `lib.rs` 非公開のため統合テスト不可
- Ctrl+↑↓ バグは過去にも複数回再発しているのに回帰ガードがない

## フェーズ分割

### Phase A: IndexerManager テストコンストラクタ ✅ 完了

`src/indexer_manager.rs` に `IndexerManager::new_at(data_dir, favorites, speed)` を追加。
`FtsMetaDb::open_at` / `FtsIndex::open_at` を呼んで tempdir 配下で動かす。

内部共通本体を `new_with_stores` に抽出したので、`new` と `new_at` でロジックは共有される。

### Phase B: 共通テストハーネス + メタ索引 E2E ✅ 完了

`tests/common/mod.rs` に以下を用意:

- `FixtureRoot` — `TempDir` ラッパ (`mkdir`, `path()`)
- `write_png_with_text(path, parameters)` — A1111 `parameters` tEXt 付き 1x1 PNG を生成
- `write_png_plain(path)` — メタなし PNG
- `make_favorite(name, path)` — `auto_index_metadata=true` の FavoriteEntry
- `start_indexer_at(data_dir, favs)` — `IndexerManager::new_at` をスピード Low で起動
- `wait_scan_done(mgr, fav_id)` — 初期スキャン完了ポーリング
- `wait_meta_contains` / `wait_meta_absent` — fts_meta.db の行の出現/消滅待ち
- `collect_search_hits(mgr, query, favs)` — `spawn_search` + `Done` まで drain
- `wait_for_search_hits(mgr, query, favs, predicate, timeout)` —
  Tantivy reader reload ラグを吸収する polling ラッパ
- `run_search_expecting_done` — reject reason 検証用

`tests/search_metadata_e2e.rs` に 9 本 (8 pass / 1 ignore):

| テスト | カバー内容 |
| --- | --- |
| `initial_scan_hits_embedded_prompt` | PNG tEXt → ingest_text → Tantivy bigram → post-filter |
| `initial_scan_hits_by_filename` | ファイル名だけの索引 |
| `watcher_picks_up_new_file` | **notify-rs で新規追加が反映** |
| `watcher_picks_up_deletion` | **notify-rs で削除が反映** |
| `watcher_picks_up_rename` | `#[ignore]` — [既知バグ](#発見したバグrename-旧パスが残る) |
| `search_respects_favorite_filter` | favorite_id フィルタが漏れないこと |
| `short_ascii_query_is_rejected` | 2 文字以下 → `TooShort` |
| `not_only_query_is_rejected` | NOT-only クエリ → `NotOnly` |
| `empty_favorite_list_returns_complete` | 対象 0 件 → 即 Complete (全件検索への誤動作防止) |

タイムアウトは `FS_EVENT_TIMEOUT = 8s` (Windows `ReadDirectoryChangesW` + 500ms debounce +
ingest commit を考慮)。

### Phase B': 名前索引 E2E ✅ 完了

master に `NameIndexSupervisor` が入った 2026-04 に着手。`tests/search_name_e2e.rs`
に 6 本 (全 pass):

| テスト | カバー内容 |
| --- | --- |
| `initial_bulk_indexes_folders_and_zips` | 初期バルクでフォルダ / ZIP を拾う |
| `watcher_indexes_new_subfolder` | **notify-rs で新規フォルダが追加索引** (v0.8.0 新挙動) |
| `watcher_indexes_new_zip` | **notify-rs で新規 ZIP が追加索引** |
| `watcher_removes_deleted_folder` | 削除フォルダが索引から消える |
| `two_supervisors_complete_in_parallel` | Tantivy 制約がない SQLite ならではの真並列動作 |
| `query_supports_exclude_token` | `-keyword` 除外構文が SQL NOT LIKE に載ること |

`SearchIndexDb::open_at(path)` を追加 (`FtsMetaDb::open_at` / `FtsIndex::open_at` と同形式)。
共通ハーネスに追加したヘルパ:
- `start_name_index_at(data_dir, fav) -> (Arc<SearchIndexDb>, NameIndexSupervisorHandle)`
- `wait_name_scan_done(handle)`
- `name_index_search(db, q, roots)` / `wait_for_name_index_hits(...)`

将来の追加候補 (優先度低):
- ZIP 内エントリの fts メタ索引 (現在の `ingest_worker` は ZIP をファイル名のみ ingest)
- 手動再構築 (`FullRescan` cmd) の挙動検証

### Phase C: App-level E2E (Ctrl+G / Ctrl+↑↓)

**Phase C 下準備 + 起動ロジック回帰テスト 完了 (2026-04)**。
eframe::Frame モックを伴うフルスタック (update() ループ経由) は未着手 (次のマイルストーン)。

#### 完了した下準備

1. **`data_dir::set_test_override(Option<PathBuf>)`** — `Mutex<Option<PathBuf>>` ベース。
   `data_dir::get()` は `TEST_OVERRIDE` → `DATA_DIR` (本番 OnceLock) → `default()` の
   順で評価する。プロセス全体のグローバルなので Phase C テストは `PHASE_C_LOCK` で
   直列化する必要あり。

2. **`App::new_for_test(AppTestConfig)`** (`#[cfg(test)]`) — TempDir と settings を
   受け取り `App::default()` の経路をそのまま通す軽量コンストラクタ。font/theme/dpi は
   設定しない、`spawn_initial_name_index_supervisors` は呼ばない (必要なテストが明示的に呼ぶ)。
   supervisor スレッドの Drop 順序: App → OverrideGuard → TempDir で file handle を先に
   閉じて Windows の使用中削除エラーを回避する。

3. **`PHASE_C_LOCK` + `OverrideGuard`** (`src/app.rs::phase_c_support`) —
   テスト間の data_dir 干渉を防ぐ serialization + panic-safe override clear。
   Phase C の全 test モジュール (`phase_c_key_tests` / `phase_c_drill_nav_tests` /
   `phase_c_drill_address_tests`) が **同じ** `PHASE_C_LOCK` を共有するので、
   別モジュール同士の並列実行でも data_dir override が干渉しない。

#### 完了したテスト (8 本、`src/app.rs::phase_c_key_tests`)

| テスト | カバー内容 |
| --- | --- |
| `new_app_has_no_search_bar_open` | 起動直後は全検索バー閉じている |
| `open_local_metadata_search_activates_only_ctrl_f` | Ctrl+F 起動で Ctrl+F のみ active |
| `open_favsearch_activates_only_ctrl_s` | Ctrl+S 起動で Ctrl+S のみ active |
| `open_global_search_activates_only_ctrl_g` | Ctrl+G 起動で Ctrl+G のみ active |
| `ctrl_f_closes_ctrl_s_and_ctrl_g` | Ctrl+F が他 2 つを閉じる |
| `ctrl_s_closes_ctrl_f` | Ctrl+S が Ctrl+F を閉じる |
| `ctrl_g_closes_ctrl_f` | Ctrl+G が Ctrl+F を閉じる |
| `at_most_one_search_bar_ever_active` | F→S→G→F→G→S→F の 7 段階で常に active ≤ 1 |

2026-04 ユーザー報告「検索バーが 2 つでることがある」回帰ガードとして機能。

#### 残タスク (次マイルストーン: フルスタック UI テスト)

フルキーストローク → UI 応答のフルスタックは `eframe::Frame` モックが必要で egui_kittest
の `Harness::builder().build_eframe(|cc| ...)` を使う方向。以下は未着手。

#### 必要な下準備

1. **`App` を lib crate に公開する**。現状 `main.rs` に `mod app`, `mod global_search_ui`,
   `mod ui_main`, `mod ui_fullscreen`, `mod ui_adjustment_panel`, `mod ui_analysis_panel`,
   `mod ui_erase`, `mod ui_metadata_panel` などが private で並んでいる。これらを
   `src/lib.rs` に `pub mod` として移す。main.rs からは `use mimageviewer::app::App;` の形に。

   影響:
   - `app` が lib 公開になることで `fn()` / `pub(crate)` アクセス制御の再整理が必要
   - 内部で `#[path = "..."]` を使っている箇所はそのまま動くはず
   - 見積: 1〜2 日 (実際に build を回して依存の再配置が必要)

2. **`App::new_for_test(cc, config: TestConfig)` を追加**:
   ```rust
   pub struct TestConfig {
       pub data_dir: PathBuf,
       pub favorites: Vec<FavoriteEntry>,
       pub skip_pdf_worker: bool,   // PDFium ワーカー子プロセスの起動を skip
       pub skip_susie_worker: bool, // Susie 32bit ワーカーの起動を skip
       pub skip_ai_runtime: bool,   // ONNX Runtime の初期化を skip
   }
   ```
   本体の `default()` と同じ流れだが、各 DB open を `*::open_at(data_dir.join(...))` に
   置き換え、workers/runtime は conditionally skip。

3. **`data_dir::set_for_test()` を追加** (必須)。`rotation_db`, `rating_db`, `spread_db`,
   `archive_cache`, `search_index_db` は現状 `open()` が `data_dir::get()` 直叩きで、
   全部に `_at` を追加するより `DATA_DIR: OnceLock` をテスト前に `.set()` する方が
   工数少ない。ただし OnceLock は 1 プロセス 1 回なので、テストバイナリ 1 ファイル
   に 1 つの data_dir しか使えない制約あり (通常は問題ない)。

#### テストの形

`tests/app_keys_e2e.rs` (egui_kittest の `Harness::builder().build_eframe()` を使用):

```rust
#[test]
fn ctrl_g_opens_search_bar_and_enters_query() {
    let fixture = setup_with_indexed_favorite();
    let mut harness = Harness::builder().build_eframe(|cc| {
        Box::new(App::new_for_test(cc, fixture.test_config()))
    });
    // 初期スキャン完了待ち
    harness.run_steps_until(|app| app.indexer_ready(), 8.secs);

    // Ctrl+G
    harness.key_press_modifiers(Modifiers::CTRL, Key::G);
    harness.run();
    assert!(harness.query::<TextEdit>().is_focused());

    // キーワード入力
    harness.type_text("mountain");
    harness.run();
    // 結果が出るまで待つ
    harness.run_steps_until(|app| app.global_search.items.len() > 0, 8.secs);
    assert!(harness.grid_contains_path_substr("a.png"));
}

#[test]
fn ctrl_down_navigates_to_next_folder() {
    // fixture: /root/a/, /root/b/, /root/c/ に画像 1 枚ずつ
    let fixture = setup_folder_tree();
    let mut harness = Harness::builder().build_eframe(|cc| {
        Box::new(App::new_for_test(cc, fixture.test_config_with_folder(fixture.a())))
    });
    harness.run_steps_until(|app| app.items_loaded(), 5.secs);

    harness.key_press_modifiers(Modifiers::CTRL, Key::ArrowDown);
    harness.run_steps_until(|app| app.current_folder == Some(fixture.b()), 5.secs);

    harness.key_press_modifiers(Modifiers::CTRL, Key::ArrowDown);
    harness.run_steps_until(|app| app.current_folder == Some(fixture.c()), 5.secs);
}

#[test]
fn ctrl_updown_within_search_results_iterates_flat_list() {
    let fixture = setup_with_multi_folder_indexed_favorite();
    // Ctrl+G → "keyword" → Enter で DrilledInto ビューに入る
    // Ctrl+↓ で検索結果の次エントリへ飛ぶことを検証
    ...
}
```

#### 想定される落とし穴

- **wgpu デバイス**: `egui_kittest` の `build_eframe` は wgpu softraster を使う。
  `wgpu-core-deps-windows-linux-android` が効いて動くはずだが、CI では実 GPU が
  ないのでモック化が必要になるかもしれない。現時点ではローカルのみで OK。
- **フルスクリーンビューポート**: `show_viewport_immediate` は kittest 未対応。
  Ctrl+↑↓ の fullscreen モードは別アプローチが要る (`favsearch_ctrl_nav` を
  単体 unit test に切り出すか、ロジックを pure function 化する)。
- **非同期 DFS**: `start_folder_nav` は worker thread + `poll_folder_nav` で非同期。
  `harness.run_steps_until` で polling する時間バジェットを十分取ること。
- **IME 状態**: TextEdit に `type_text` すると IME 状態の管理 (CLAUDE.md §IME対応)
  が噛むので、`App::ime_input_active()` が false になる条件を `run_steps_until` で
  待つ必要あり。

#### 見積

- Phase C 下準備 (公開化 + new_for_test + data_dir test hook): **1〜2 日**
- Phase C テスト本体 (Ctrl+G, Ctrl+↑↓ 各 3〜5 本): **半日〜1 日**
- トータル **2〜3 日**

master の NameIndexSupervisor が着地して、さらに検索バー系のバグ退治が落ち着いた
タイミングで着手するのが無難。先にやると master 側の conflict が激しい。

## 発見したバグ: rename 旧パスが残る

`tests/search_metadata_e2e.rs::watcher_picks_up_rename` を書いて回帰ガードを
設定したところ、**現行実装で fail することを確認**:

### 再現

1. favorite ルートに画像を置いて初期スキャン完了
2. `std::fs::rename(old, new)` で画像をリネーム
3. 旧パスが `fts_meta.db` から消えず、Tantivy index にも残る
4. 検索で旧パスが結果に出続ける

### 根本原因

Windows `ReadDirectoryChangesW` は rename を `Modify(Name(From))` + `Modify(Name(To))`
の 2 イベントで届ける。現行の [`search_watcher::absorb_event`](../src/search_watcher.rs) は:

```rust
EventKind::Create(_) | EventKind::Modify(_) => ChangeKind::Upsert,
```

と、`Modify(_)` を一律 `Upsert` に変換している。結果、**rename 元パス** も `Upsert`
としてダウンストリームに流れる。

[`indexer_supervisor::apply_single_change`](../src/indexer_supervisor.rs) の Upsert 分岐は
`build_candidate_from_path(&path, key)` を呼ぶが、`std::fs::metadata` が失敗する
(ファイルが存在しないため) ので `None` を返して **silently no-op**。旧パスは
`fts_meta` / Tantivy どちらにも残ったまま。

### 修正案

**案 1**: `search_watcher::absorb_event` で `ModifyKind::Name(RenameMode::From)` だけ
`ChangeKind::Remove` に変換する。
```rust
EventKind::Modify(ModifyKind::Name(RenameMode::From)) => ChangeKind::Remove,
EventKind::Modify(_) | EventKind::Create(_) => ChangeKind::Upsert,
```

**案 2**: `apply_single_change` の `Upsert` 分岐で、candidate が作れなかった場合は
`Remove` にフォールバックする。
```rust
ChangeKind::Upsert => {
    let Some(cand) = build_candidate_from_path(&path, key.clone()) else {
        // ファイルが無ければ削除扱い (rename 元・削除済みに共通で効く)
        apply_remove(session, writer, ..., key);
        return;
    };
    ...
}
```

案 1 は意図が明確だが `notify-rs` の `RenameMode` セマンティクスに依存し platform
ごとに挙動差がある。案 2 は保険的で副作用がわかりやすい。**両方入れる** のが実用的:
案 1 で意図を明示、案 2 で notify-rs のエッジケース耐性を持たせる。

修正後は `tests/search_metadata_e2e.rs::watcher_picks_up_rename` の `#[ignore]` を
外すこと。
