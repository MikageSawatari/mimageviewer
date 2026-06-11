# 検索アーキテクチャ

mimageviewer の検索システム (Ctrl+S / Ctrl+F / Ctrl+G + タグ機能) の全体像。
修正作業の前に必ず読むこと。実装詳細・個別の仕様選択の掘り下げは
[search-expansion-design.md](search-expansion-design.md) と
[tag-feature.md](tag-feature.md) を参照。

---

## 1. 全体像

### 1.1 3 つの検索モード

検索は「コンテナを探す」「アイテムを探す」「今いる場所を絞る」の 3 軸に整理
されている。設計の意図と全体方針は
[search-container-item-redesign.md](search-container-item-redesign.md) が正典。

| ショートカット | 呼称 | 用途 | スコープ | 実装経路 |
| --- | --- | --- | --- | --- |
| **Ctrl+S** | コンテナ検索 | フォルダ / ZIP / PDF を名前で横断検索 | お気に入り配下 (再帰) | コンテナ索引 `search_index.db` (SQLite LIKE) |
| **Ctrl+F** | 現在地フィルタ | 現グリッドの表示中アイテムを名前 / メタ情報で絞り込み | 現在グリッド (非再帰・索引なし) | worker 上の on-demand 判定 |
| **Ctrl+G** | アイテム検索 | 画像 / PDF / 動画を名前 / タグ / EXIF / AI プロンプト等で横断検索 | お気に入り配下 (再帰) | アイテム索引 (Tantivy bigram) 候補絞り込み + STORED 原文 post-filter の streaming |

- **動画はコンテナ索引 (Ctrl+S) の対象外** — 動画はアイテムなので Ctrl+G で扱う。
- **ZIP はアイテム索引 (Ctrl+G) の対象外** — ZIP はコンテナなので Ctrl+S で扱う。
  ZIP 内画像のメタ検索はどのモードでも行わない (§4.6)。
- **Ctrl+F は索引を使わない** — 現グリッドの表示中アイテム (構造アイテム含む) を
  その場で判定する。構造アイテム (Folder / ZIP / PDF) もファイル名で一貫して
  絞り込む (§5.2)。

3 つは UI 上で排他表示される (同時に 2 本開かない)。回帰ガードは
`src/app.rs::phase_c_key_tests`。

### 1.2 クエリ構文 (全モード共通)

[src/search_query.rs](../src/search_query.rs) が担当:

- 空白区切りでトークン化 (AND 結合が既定)
- `"..."` でフレーズ (括られた内部はそのまま連続マッチ)
- `-word` で NOT (除外)
- `□OR` トグルで include トークンを OR 結合に切替 (NOT は常に AND)
- 正規化は [src/search_norm.rs](../src/search_norm.rs) の
  `normalize_for_match` (= `to_lowercase`)。**ingest 側・クエリ側・post-filter 側で
  必ずこの 1 関数を通す**。片方だけ NFKC を入れると偽ヒット / 検索漏れが出る。

最小クエリ長:

- CJK を 1 文字でも含む → 2 文字以上
- 英数字のみ → 3 文字以上 (bigram の爆発的ヒット回避)
- NOT-only (正トークン 0) は **Ctrl+G では拒否** (全件 scan になるため)。Ctrl+F は
  対象件数が小さいので許可。

### 1.3 タグ機能との関係

[tag-feature.md](tag-feature.md) 参照。タグは XMP `dc:subject` に `#プレフィックス付き`
要素として書き込み、ingest 時に [fts_index](../src/fts_index.rs) の `tags` フィールドへ
インデックスする。Ctrl+G の「検索対象=タグ」フィルタや `#原神` クエリはこの
フィールドをヒットさせる。

---

## 2. モジュールマップ

### 2.1 クエリ層 (UI スレッドが呼ぶ)

| モジュール | 役割 |
| --- | --- |
| [search_query.rs](../src/search_query.rs) | トークナイザ + AST + `matches` / `matches_with_mode` / `decide_partial` / `MatchMode` |
| [search_norm.rs](../src/search_norm.rs) | `normalize_for_match(s) -> String` — 全経路で共有する唯一の正規化関数 |
| [global_search.rs](../src/global_search.rs) | Ctrl+G のクエリワーカー (streaming、Searcher snapshot 固定) |
| [global_search_ui.rs](../src/global_search_ui.rs) | Ctrl+G の検索バー・drill-down ビュー・結果集約 |

Ctrl+S / Ctrl+F の UI は [ui_main.rs](../src/ui_main.rs) の
`render_favsearch_bar` / `render_search_bar`、実行部は [app.rs](../src/app.rs) の
`execute_favsearch` / `run_metadata_search`。

### 2.2 インデックス層 (バックグラウンド)

| モジュール | 役割 |
| --- | --- |
| [indexer_manager.rs](../src/indexer_manager.rs) | 全お気に入りの `SupervisorHandle` を束ね、Ctrl+G ワーカー spawn・進捗 snapshot・`sync_with_favorites`・App 終了時の停止を統括 |
| [indexer_supervisor.rs](../src/indexer_supervisor.rs) | **1 お気に入り 1 本**。初期スキャン + notify-rs 監視 + 差分 ingest を統括 (メタ索引側) |
| [indexer_progress.rs](../src/indexer_progress.rs) | Supervisor → UI 用の `ProgressReporter` (Mutex で包まれた短文 `current_activity`) |
| [search_walker.rs](../src/search_walker.rs) | 起動時の再帰 walk + 3-way diff (FS / `fts_meta.db` の突き合わせ) |
| [search_watcher.rs](../src/search_watcher.rs) | notify-rs `ReadDirectoryChangesW` ラッパ + 500ms debounce |
| [ingest_worker.rs](../src/ingest_worker.rs) | メタ抽出 + Tantivy buffer + バッチ commit + fts_meta 状態遷移 |
| [ingest_text.rs](../src/ingest_text.rs) | `PerSourceText` (filename / exif / xmp_tweet / png_prompt / pdf_meta / video_meta / tags / sidecar) ビルダー |
| [external_metadata.rs](../src/external_metadata.rs) | 外部メタデータサイドカー (画像と同名 .json/.txt) の検出・値抽出・差分署名・逆引き (§4.10) |
| [name_index_supervisor.rs](../src/name_index_supervisor.rs) | Ctrl+S 用 **名前索引 supervisor** (初期バルク + notify-rs 追従) |
| [name_bulk_indexer.rs](../src/name_bulk_indexer.rs) | Ctrl+S 用 初期バルクスキャンの本体 |
| [io_semaphore.rs](../src/io_semaphore.rs) | `GlobalIoSemaphore` — UI / PDF / サムネ / インデクサ横断の I/O 同時実行制御 |

### 2.3 ストレージ層

| モジュール | 役割 |
| --- | --- |
| [fts_index.rs](../src/fts_index.rs) | Tantivy 0.26 ラッパ。`IndexDoc` / `Fields` / `QueryFilters` / `build_bigram_and_query` / `search_page` |
| [fts_meta.rs](../src/fts_meta.rs) | `fts_meta.db` (SQLite) ラッパ。ファイル単位の管理メタ (path / mtime / size / status=Ok\|Failed / index_generation)。検索原文は持たず Tantivy STORED に集約 |
| [search_index_db.rs](../src/search_index_db.rs) | Ctrl+S 用 `search_index.db` (フォルダ / ZIP / PDF / 動画名) |

### 2.4 タグ機能

| モジュール | 役割 |
| --- | --- |
| [tag_ops.rs](../src/tag_ops.rs) | `#タグ` 要素の Bag 操作ヘルパ (add / remove / clear-hash-prefixed) |
| [tag_write_worker.rs](../src/tag_write_worker.rs) | UI → XMP 書き込み worker。書込み成功後に共有 `IndexWriter` 経由で即時 Tantivy 反映 |
| [xmp_writer.rs](../src/xmp_writer.rs) | 既存メタを保持したままの dc:subject atomic 書換 (JPEG / PNG / WebP) |

---

## 3. 永続化ストア

すべて `%APPDATA%/mimageviewer/` 配下。

| パス | 目的 | 書き手 | 注記 |
| --- | --- | --- | --- |
| `settings.json` | `FavoriteEntry { id, name, path, auto_index_{structure,metadata,thumbs} }` + `tags: Vec<TagDef>` | [settings.rs](../src/settings.rs) | UUID が欠けている行は起動時に発行し書き戻し |
| `search_index.db` | Ctrl+S 用フォルダ/ZIP/PDF/動画名 index (SQLite LIKE で引く) | `search_index_db.rs` | `indexed_by_auto` 列で手動/自動エントリを区別 |
| `fts_index/` | Tantivy index ディレクトリ (複数 segment ファイル + meta.json)。**INDEX_VERSION=5 以降は per-source `*_text` フィールドが STORED で原文を保持** | `fts_index.rs` → IngestSession / tag_write_worker | schema 変更は `schema_is_stale` (STORED 必須含む) で検出し全消去 + 再構築 |
| `fts_meta.db` | `files(path PK, favorite_id, kind, mtime, size, indexed_at, index_version, index_generation, status)` — INDEX_VERSION=5 で `*_norm` 列群を撤去し管理メタ専用に縮小 | `fts_meta.rs` | `INDEX_VERSION` を bump すると `needs_rebuild` が `*_norm` 残存も検出して全再構築を促す |

**パスキー正規化**: Windows の大文字小文字非区別と区切り文字混在に備え、
fts_meta.db / Tantivy / 起動時 diff・Ctrl+F on-demand 判定の全経路で `normalize_path`
(= lowercase + `/` 区切り + ZIP 内エントリは `<zippath>\u{1F}<entry>`、
separator は `search_norm::ZIP_ENTRY_SEP` = U+001F Unit Separator) を通す。
新しい lookup 経路を追加するときも同じ正規化を通すこと。
`!` を separator に戻してはいけない (通常ファイル名と衝突する。INDEX_VERSION=4 で廃止)。

---

## 4. インデクサパイプライン

### 4.1 アイテム索引 (Ctrl+G 用)

画像 / PDF / 動画を、ファイル名・タグ・EXIF・AI プロンプト等で横断検索する
ための索引。Ctrl+G が使う。Ctrl+F (現在地フィルタ) はこの索引を使わず、
表示中アイテムを on-demand に判定する (§5.2)。

```
App 起動
  └─ IndexerManager::new
       ├─ FtsMetaDb + FtsIndex を open (schema 不一致なら全再構築)
       ├─ 起動時 reconciliation (§4.3) を **同期** で実行
       └─ auto_index_metadata=true のお気に入りごとに
            IndexerSupervisor::spawn  (1 お気に入り 1 本、以降ずっと常駐)

IndexerSupervisor (スレッド 1 本):
  1. FsWatcher 起動 (notify-rs + 500ms debounce)
  2. search_walker::scan  …… 初期スキャン (3-way diff)
         FS にあり DB になし → ingest queue
         DB にあり FS になし → delete queue
         両方あり mtime/size 差 → 再 ingest queue
  3. IngestSession::run   …… Tantivy First 書き込み順序 (§4.2) で反映
  4. 以降 watcher イベント (DebouncedChange) を受け取り小刻みに 3 と同じ処理
  5. App drop で cancel + FsWatcher drop + thread join

IngestSession の writer は IndexerManager が保有する
Arc<Mutex<IndexWriter>> を共有する (Tantivy の writer は Index あたり 1 本制約)。
tag_write_worker も同じ writer を共有するため、タグ書き込みと通常 ingest の
commit は干渉しない。
```

### 4.2 書き込みプロトコル (Tantivy First, INDEX_VERSION=6)

`fts_meta.db` (SQLite) と Tantivy index は別ストレージ。INDEX_VERSION=6 から
**Tantivy First** に統一: Tantivy commit が成功したフレームでのみ SQLite を
更新する。SQLite の `status` は `Ok` / `Failed` の 2 値だけで、Pending / Tombstone
の中継状態は廃止した。検索 post-filter (`fts_meta` への SQLite SELECT) も廃止。

#### Upsert (ingest)

```
(1) IndexDoc を構築 (メタ抽出 + norms 生成、SQLite には触れない)
(2) Tantivy writer に delete(path) + add_document を push (バッファに積む)
(3) バッチ境界 (100 件 or 5 秒) で IndexWriter::commit() + reader reload
(4) commit 成功後に fts_meta.db UPSERT status=Ok, index_generation += 1
    (ここで失敗してもログだけ。次回起動の walker 3-way diff で再 ingest される)
```

#### Delete

```
(1) Tantivy writer に delete_term(path) を push
(2) バッチ境界で commit + reader reload
(3) commit 成功後に fts_meta.db DELETE
    (失敗時は SQLite に行残る → 次回 walker が「FS なし + DB あり」で再 delete)
```

#### クラッシュからの復旧

| クラッシュ位置 | Tantivy | SQLite | 復旧経路 |
|---|---|---|---|
| (1) 前 / (2) 前 | 古い | 古い | walker は何も拾わない |
| (3) commit 中 | 古い or 新 | 古い | walker が「FS あり + DB 古い」→ 再 ingest |
| (3) と (4) の間 | 新 | 古い | walker が「FS あり + DB 古い」→ 再 ingest (Tantivy 上書き) |
| (4) 後 | 新 | 新 | OK |

中間状態 (Tantivy だけ新) で検索結果が一瞬古い text を返し得るが、削除直後の
削除済みファイルが結果に出るのと同じ "短い窓" として許容する (実害はサムネイル
読み込み失敗で気付ける)。

### 4.3 起動時 reconciliation

`IndexerManager::new` が supervisor spawn 前に同期実行する:

- `status=Failed` の行 → Tantivy delete_term + SQLite delete_paths
  (legacy v5 DB から migrate された Pending/Tombstone もここに集約される)
- `index_version` 不一致 or `fts_index/` schema 不一致 → 全再構築

通常は数十〜数百行程度で 100ms 以下。大量なら supervisor 起動は待たされるが、
writer 競合防止のため同期実行する方が安全 (非同期化すると supervisor と
reconcile が `IndexWriter` を奪い合って失敗する)。

VACUUM 等の housekeeping は起動経路から外し、全 supervisor が初期 scan を完了して
idle になった最初のフレームで `spawn_housekeeping` から別スレッドで走らせる。

### 4.4 コンテナ索引 (Ctrl+S 用)

アイテム索引とは別系統。フォルダ / ZIP / PDF を「名前で探せるコンテナ」として
登録する:

```
NameIndexSupervisor (1 お気に入り 1 本):
  1. name_bulk_indexer::run_bulk_name_index  …… フォルダ / ZIP / PDF の再帰列挙
  2. SearchIndexDb::upsert_children で差分反映 (INSERT OR REPLACE)
  3. FsWatcher でイベント受信 → name_index_supervisor::apply_single_change が
     try_exists() ベースで判定し、新規ディレクトリなら subtree 再帰 upsert、
     削除なら ancestor chain prune + delete_subtree を実行 (詳細は §4.5)
```

- アイテム索引側と違い書き込み先が SQLite 単独なので複数お気に入りの supervisor は
  真の並列で動ける (Tantivy writer 単一制約がない)。
- 画像個別は扱わない (画像の名前はアイテム索引 / Ctrl+G 側で拾う)。
- **動画はコンテナ索引の対象外** — 動画はコンテナではなくアイテムなので、
  `classify_name_index_kind` は動画拡張子に対して `None` を返し、`SearchIndexDb` の
  `kind` 列にも `VideoFile` を書かない。`search()` は `kind=None` ("すべて") でも
  クエリ側で `VideoFile` を弾くので、旧バージョンが書いた行が残っていても
  Ctrl+S 結果には出ない (docs/search-container-item-redesign.md §4.2 / task#4)。

### 4.5 FsWatcher と debounce、`apply_single_change` の挙動

[search_watcher.rs](../src/search_watcher.rs):

- notify-rs `ReadDirectoryChangesW` を `RecursiveMode::Recursive` で張る
- 500ms ウィンドウで同一 path のイベントをまとめ、`DebouncedChange` に畳む
- rename は Windows で `Modify(Name(From))` + `Modify(Name(To))` として届くため
  `absorb_event` が From を `ChangeKind::Remove`、To を `ChangeKind::Upsert` に
  分ける (単一の `Modify(_) → Upsert` にすると rename 元が削除されず残る)。

#### `apply_single_change` の判定経路 (v0.9.x B4 改訂、Codex レビュー反映)

旧実装は「changed_path の **親フォルダ** を `read_dir` して `upsert_children`」
の 1 経路だけだった。これだと:

- 起動後に深いサブツリーをコピー / 移動した場合、watcher overflow や個別イベント
  欠落で「上位イベントだけ届く」と深い ZIP/PDF/動画が索引に入らない (Codex P1)
- 深い `marker.zip` の Remove イベントだけ届くと、`new_top/mid/deep` の Folder 行が
  stale 残留する (`upsert_children` の DELETE は親直下しか触らない)
- `read_dir(parent)` の `Err` を「親まるごと削除」と即断していたため、アクセス拒否 /
  一時ロック / NAS 切断で正当な行が消える事故が起き得る

これを以下の経路に再設計:

1. **favorite 境界チェック**: `changed_path` が `favorite_root` 配下でなければ no-op
2. **`changed_path.try_exists()` を唯一の削除判定にする** (`kind` はヒント / ログ用):
   - **`Ok(false)`** → ancestor chain を辿り、`favorite_root` 手前まで親を順に確認。
     `Ok(false)` で繋がる連鎖の **最も浅い missing 祖先** を `delete_subtree` の対象に
     (`new_top` 全部消えたなら `new_top` 自体を一発 prune)。
     `changed_path == favorite_root` で `Ok(false)` のときは `clear_for_favorite` で全消し
   - **`Ok(true)`** → `scan_start = next_write_stamp()` を **最初に** 取得 (post-scan
     prune が `changed_path` 自身を stale と誤判定しないため)。
     `changed_path != favorite_root` なら parent refresh
     (`upsert_children` で sibling 整合)。`is_dir()` なら `run_subtree_scan` で
     `changed_path` から再帰的に各フォルダ直下を `upsert_children`。scan が
     `SubtreeScanOutcome::Completed` の場合のみ `prune_stale_under_subtree` を呼び、
     fast delete → recreate (子孫が減る) ケースの stale 孫行を掃除する。
     `changed_path == favorite_root` のときは parent refresh を **skip**
     (`favorite_root.parent()` は favorite 配下外なので、そこを upsert すると sibling
     フォルダを fav の行として誤投入する事故になる)
   - **`Err(e)`** → アクセス拒否 / 一時ロック / NAS 切断などの曖昧状態。
     `crate::logger::log` に warn を残し、**破壊的 cleanup も再帰 upsert も
     一切走らせない**。次回 watcher イベント / 次回起動時 walker 3-way diff が拾い直す

3. **subtree scan の不完全観測時は post-scan prune を skip**:
   `run_subtree_scan` は cancel / read_dir error / upsert error を `SubtreeScanOutcome::Cancelled`
   または `SubtreeScanOutcome::Errored` で返す。`Completed` 以外では `prune_stale_under_subtree`
   は実行されない (フル scan が `cancelled` 時に prune skip するのと同じポリシー)

`SearchIndexDb` 側の関連メソッド ([src/search_index_db.rs](../src/search_index_db.rs)):

- `delete_subtree(favorite_root, root_path)`: `root_path` 自身と配下を `favorite_root`
  スコープで一括削除。境界判定は `LIKE` を使わず `subtree_bounds()` ヘルパで構築した
  trailing-slash 付き prefix を `substr` 比較するので、path に `%` `_` 等の wildcard 文字が
  含まれていても誤削除されない。
- `prune_stale_under_subtree(favorite_root, root_path, cutoff)`: 同じ境界判定 +
  `updated_at < cutoff` を AND。subtree scope の stale 行のみ削除。`Completed` outcome の
  ときだけ呼ぶ (不完全観測で正当な行を消さないため)。

SMB / NAS では `ReadDirectoryChangesW` が発火しないケースがあるので、将来は
ポーリング fallback を足す想定 (§7.2)。現状は手動再構築ダイアログで代用。

### 4.6 ZIP 対応のスコープ

**ZIP はコンテナであり、アイテム索引 (Ctrl+G) の対象外。** ZIP ファイル名での
横断検索はコンテナ索引 (Ctrl+S) が担う。

- ZIP ファイル自体: アイテム索引には登録しない。`search_walker` は ZIP を
  `fs_map` に入れない (`CandidateKind::Zip` を作らない) ので、過去バージョンが
  登録した ZIP doc は 3-way diff で「FS になし + DB あり」と判定され `to_delete`
  に落ちて Tantivy から消える (docs/search-container-item-redesign.md §3.2 / task#1)。
- ZIP 内画像: **どのモードでもメタ検索しない**。アイテム索引は ingest せず、
  Ctrl+F も ZIP 表示中はファイル名フィルタのみ (§5.2、同 §4.1.2)。ZIP は seek
  不可でエントリごとの伸張コストが高く、生成画像を ZIP 保管する需要も低いため。
- `fts_index` schema の `zip_entry` フィールドや `global_search_ui` の
  `split_zip_hit_path` / `build_drilled_zip_items` は将来 ZIP 内アイテム ingest を
  行う場合の足場として残してあるが、現状の経路からは到達しない。
- ZIP 内画像のメタ検索が必要になったら、walker の ZIP 内列挙 + ZIP 専用 ingest
  context を伴う独立した後続プロジェクトで扱う。

### 4.7 PDF 対応のスコープ

- PDF ファイル本体: ファイル名 + PDFium document info (Title / Author / Subject /
  Keywords) のみ。`pdf_meta_text` フィールドへ。
- **本文テキストは v1 で対象外**。OCR 付き PDF (マンガのルビ等) は 1 ページ数 KB
  規模で bigram 索引を肥大化させ、誤認識ノイズで偽ヒットが増える。opt-in で
  別 index (`pdf_fts_index/`) に分離する案は v1.x 以降に残す。

### 4.8 動画対応のスコープ

- 動画ファイル本体: ファイル名 + FFmpeg が読めるコンテナメタデータ (title /
  artist / URL / description / comment / chapter title 等) を `video_meta_text` へ。
- 動画タグ: 本体を直接書き換えず、`<video.ext>.xmp` サイドカーの `dc:subject`
  を `tags` フィールドへ。Ctrl+F / Ctrl+G の「タグ」対象で画像タグと同じように
  検索できる。
- フレーム内容 / 音声文字起こし / チャプター以外の本文抽出は対象外。動画再生や
  サムネイル抽出とは独立した低頻度のメタ読み取りだけを行う。

### 4.9 タグ書き込みと即時反映 (INDEX_VERSION=5)

[tag_write_worker.rs](../src/tag_write_worker.rs) は UI からの Toggle / Clear 要求を
1 ファイルずつ serial に処理し、以下のフローで進める:

1. XMP atomic rewrite (`xmp_writer::apply_tag_op`)
2. `fts_meta.get(path)` で管理メタ (kind / mtime / size) を取得 + `status == Ok`
   なら次へ (Pending 中は ingest に任せて skip — race 回避)
3. `fts.reload_reader()` で最新 commit を含む snapshot を取り、`find_doc_by_path` +
   `doc_per_source_text` で既存 STORED 値を読み取る
4. `tags` フィールドだけ差し替えて `IndexDoc` を再構築し、共有 `IndexWriter` で
   `WriterPriority::Interactive` upsert
5. 32 件 or 500ms でバッチ commit (reload 同期付き)

UI は commit 完了シグナルを受けたタイミングで toast を出す (commit より前に
toast を出すと直後の Ctrl+G に新タグが出ない race がある)。

INDEX_VERSION=5 で原文が Tantivy 側に集約された影響で、`tag_write_worker` は
他ソース原文 (name / exif / xmp_tweet / png_prompt / pdf_meta / video_meta) を **保持したまま**
tags だけ差し替える必要がある。ここで stale snapshot を読むと ingest が直前に
commit した最新原文を旧値で潰してしまうため、上記 #2 / #3 の race ガードが必須。

### 4.10 外部メタデータサイドカー (画像のみ、INDEX_VERSION=8)

画像と **同名のサイドカーファイル** (`<画像名>.json` / `<画像名>.txt`) の内容を、**読み取り専用の
フリーテキスト** として `sidecar_text` フィールドへ索引する。詳細設計は
[sidecar-metadata-ingest.md](sidecar-metadata-ingest.md)。

- **mIV タグ (`tags`, `#xxx`) とは別系統**。サイドカーからタグ抽出はせず、`#` も付けない。
  検索対象フィルタの「サイドカー」(`SourceKind::Sidecar`) と「タグ」(`SourceKind::Tags`) は独立。
- **抽出**: JSON は再帰してリーフ **値のみ** 連結 (キー名は索引しない)。TXT は全文。
  `external_metadata::read_search_text` → `normalize_for_match` → `ingest_text` step 5。
- **検出**: `<full>.json` → `<full>.txt` → `<stem>.json` → `<stem>.txt` の優先順位で最初の 1 つ。
  サイズ上限 2MB、連結上限 256KB、壊れ JSON は空に倒す。
- **差分検出**: walker / supervisor は画像候補の **差分用署名** に
  `diff_mtime = max(画像 mtime, サイドカー mtime)` と
  `diff_size = 画像 size + サイドカー fingerprint` (ファイル名 + size の安定ハッシュ、無しは 0) を
  織り込む (`CandidateFile.diff_mtime` / `diff_size`)。これでサイドカーの追加・編集・削除に加え、
  優先順位プローブの結果が別ファイルに変わったケース (同 mtime/size でも) も 3-way diff で検出される。
  Tantivy doc の `mtime` は画像本体のまま (日付ソートがサイドカー編集時刻に引きずられない)。
- **監視追従**: `.json`/`.txt` の変更イベントは `indexer_supervisor::apply_single_change` が
  `external_metadata::images_for_sidecar` で兄弟画像へ逆引きして再 ingest に変換する
  (Remove なら `sidecar_text` がクリアされる)。
- **Ctrl+F**: `run_metadata_search` の Pass 2 で FS 画像のみ on-demand 読みして hay に含める。
- **設定**: 専用 ON/OFF は持たない。アイテム索引 (`auto_index_metadata`) に追従し、OFF 化は
  `purge_favorite_metadata` で `sidecar_text` ごと消える。
- **動画 / ZIP 内 / PDF ページは対象外** (動画は既存 `.xmp` サイドカーの mIV タグ経路を維持)。

### 4.11 AI 生成メタデータ (INDEX_VERSION=9)

PNG の tEXt/iTXt/zTXt と JPEG/JFIF の EXIF UserComment に埋め込まれた生成メタデータを
`png_prompt_text` フィールドへ正規化して入れる。実装は
[png_metadata.rs](../src/png_metadata.rs) に集約し、メタデータパネル、Ctrl+F、Ctrl+G
の 3 経路で同じ判別器を使う。

- `prompt` が JSON object の場合は ComfyUI として扱う。
- `parameters` が JSON object の場合は SwarmUI / Fooocus 系などの生成メタデータとして
  先に分岐し、未知の生成 JSON は生 JSON をプロンプト扱いしない。
- NovelAI は `Description` 分岐より前に判別し、`Comment.uc` などの Negative を
  検索対象へ戻さない。
- JPEG/JFIF の EXIF UserComment が AI 生成メタデータとして解釈できた場合は、
  `exif_text` 側から UserComment を除外して二重取り込みと Negative 混入を避ける。
- 検索テキスト構築は静的な AI キー一覧ではなく、判別器が実際に消費したキー
  (`consumed_keys`) を素通し除外に使う。新フォーマット追加時のキー追加漏れで
  Negative が混入する事故を防ぐため。

---

## 5. クエリ実行パス

### 5.1 Ctrl+S — コンテナ (フォルダ / ZIP / PDF) 名検索

```
UI (render_favsearch_bar)
  → execute_favsearch (spawn worker thread)
    → SearchIndexDb::search(query, favorite_roots, kind, mode)
         SQL: SELECT ... FROM entries WHERE
              include 群を mode に応じて AND / OR 結合した LIKE
              AND exclude 群の NOT LIKE
              AND favorite_root IN (...)
              AND 種別フィルタ:
                  kind=Some(k) なら kind = ?
                  kind=None    なら kind <> VideoFile
  → poll_favsearch が結果を受け取り GridItem::Folder/ZipFile/PdfFile に展開
```

- `kind` 引数は Ctrl+S バーの「種別」ドロップダウン (すべて / フォルダ / ZIP / PDF)。
- 動画はコンテナ索引の対象外 (§4.4)。`kind=None` ("すべて") でもクエリ側で
  `VideoFile` を弾くので、旧バージョン由来の VideoFile 行が残っていても結果に出ない。
- バインド順は include → exclude → favorite_roots → kind。

Tantivy は通さない。SQLite LIKE の方が対象件数 (フォルダ構造の粒度) に対して
速く、実装もシンプル。

### 5.2 Ctrl+F — 現在地フィルタ (現グリッドの表示中アイテムのみ)

```
UI (render_search_bar)
  → run_metadata_search(tokens, items, xmp_cache, fts_meta, pdf_passwords,
                        target, mode, cancel)
    Pass 1 (構造アイテム + ZIP 内画像、名前照合中心):
       - Folder / ZipFile / ConvertibleArchive … ファイル名 (basename) で照合
       - PdfFile … まずファイル名、外れたら PDF document info も見る
         (target が PDF メタを含むとき。get_document_info の IPC、
          保存済みパスワードは pdf_passwords から、PDF 境界でのみ cancel)
       - ZipImage … ファイル名のみ (ZIP を開いてメタを読む経路は持たない)
       - ZipSeparator … 付随グループに可視 ZipImage が残るときだけ表示
    Pass 2 (Image / Video、on-demand メタ):
       - target に応じて必要なメタだけ読む (PNG tEXt / JPEG EXIF UserComment AI メタ / EXIF / XMP /
         動画コンテナメタ / dc:subject)
    → 検索中は item 単位の done/total を SearchPending の atomic snapshot に反映
    → 合格 idx を HashSet<usize> に反映 (search_filter)
```

- **構造アイテムも一貫して絞り込む**: フォルダ / ZIP / PDF も「持っている次元」
  (ファイル名、PDF は document info も) で判定する。検索対象がファイル名次元を
  含まない (EXIF / タグ単独指定) なら構造アイテムは全件非表示になる。
- **PDF のページ表示中は Ctrl+F 自体を無効化** (`grid_is_pdf_pages`): 連番の
  合成ページ名を絞っても意味が無いため、ショートカットを無反応にし開いた
  バーも閉じる。
- **ZIP 表示中は検索対象をファイル名に固定** (`grid_is_zip_entries`): ZIP 内
  画像のメタ検索は行わない (§4.6)。
- 検索中表示は処理済み item 数 / 総 item 数を出す。分母・最終件数とも
  `ZipSeparator` は除外する。1 item 内のメタ読み取り中は、その item が完了するまで
  数字は進まない。
- 件数バッジは可視マッチ全体を「X/Y 件」で数える (separator は除く)。
- フィルタが効いている元フォルダでは BS / Alt+↑ / アドレスバーの ⬆ による
  親移動を no-op / disabled にする。検索結果から子フォルダへ入った後は既存どおり
  `search_filter` が解除されるため、BS で元フォルダへ戻れる。

**インデックスを使わない理由**: 対象が表示中の数十〜数千件に限定されるので、
on-demand で必要なソースだけを読む方が (a) 検索漏れゼロ、(b)「今見えている
一覧」だけに閉じられる、(c) 実装が単純。重い読み取りは worker スレッド上で
行い、UI スレッドでは実行しない。詳細は
[search-container-item-redesign.md](search-container-item-redesign.md) §4.1。

### 5.3 Ctrl+G — グローバルメタ検索 (streaming)

```
UI (global_search_ui::render_global_search_bar)
  → IndexerManager::spawn_search(query, filters, mode)
    → global_search::run (別スレッド)

  [worker] スレッド内ループ:
    0. クエリ検証 (最小長、NOT-only 禁止)
    1. 正トークンを bigram 分解し build_bigram_and_query で BooleanQuery を構築
       - include: target フィールド (filename/exif/xmp_tweet/png_prompt/pdf_meta/video_meta/tags)
         ごとの OR を各トークンの子クエリにし、mode=AND なら top-level Must で、
         mode=OR なら Should 群を 1 Must にまとめる
       - favorite_id / kind は Must (exact term の OR)
    2. FtsIndex::searcher() で Searcher snapshot を固定 (ページング中の
       重複・抜け防止: commit が走っても snapshot は古い seg を見続ける)
    3. TopDocs::with_limit(PAGE_SIZE=500).and_offset(offset) をループ:
       a. Tantivy 候補 (path, addr, score) を取得
       b. doc_text_for_target で同じ Tantivy snapshot から STORED 原文を引く
          (target で取り出すフィールドを切り替え)
       c. search_query::matches_with_mode で post-filter (phrase / NOT / AND/OR 判定)
       d. Batch 送信 (path と hit 情報を streaming)
       e. 累計 valid_hits が HARD_MAX=10_000 到達で TruncatedAtMax 終了
       f. 候補使い切りで Complete、cancel で Cancelled
    4. Done 送信

  [UI] 毎フレーム:
    → poll_global_search が try_recv ループで Batch を受信
    → 検索中は ActivityGate を bump し続け、背景インデクサの walker/ingest を
      次の処理単位で待機させる
    → push_grid_item_pending で items + thumbnails をセット拡張
    → rebuild_global_search_items が現在のビューに合わせて items を再構築:
        - Flat (一覧)      … 全ヒットを画像 / PDF / 動画として平坦に並べる
        - Aggregated (集約) … ContainerHit ごとに SearchContainer へ集計
        - DrilledInto       … 集約からコンテナを 1 つ開いた階層内絞込
      ビューは GlobalSearchState の drill / aggregate / aggregate_auto から
      導出される。ヒット数が閾値を超えると一覧 → 集約へ自動切替する
      (ユーザーが結果を操作した後はロックして勝手に切り替えない)。
    → pending が残っていれば ctx.request_repaint()
```

**post-filter が必須な理由**: Tantivy の `NgramTokenizer` は token position を
常に 0 で吐く仕様。bigram だけでは phrase `"海辺 夕焼け"` と AND の連続部分一致を
正しく判別できず、偽陽性が出る。post-filter で `normalize_for_match` 済みの原文に
対して phrase / NOT / AND を再評価することで 偽陽性を 0 にする。NOT は
Tantivy に渡すと position=0 由来で誤判定するため **post-filter 側でだけ** 評価する。

---

## 6. 設計上の主要な選択理由

### 6.1 なぜ Tantivy + bigram (NgramTokenizer 2/2) か

| 選択肢 | 不採用の理由 |
| --- | --- |
| SQLite LIKE (Ctrl+S と同方式) | 大規模 (数十万件) では線形劣化。日本語の部分一致で index が効かない |
| SQLite FTS5 unicode61 | CJK を単語区切りできない |
| SQLite FTS5 trigram | 標準 trigram は CJK で 3 文字未満がヒットしない。書き込みも遅い |
| Tantivy + Lindera (形態素解析) | 辞書 13 MB 追加。AI プロンプトの英語混在や新語 (stable-diffusion, lora 等) を分割ミスする |
| **Tantivy + bigram (採用)** | 2 文字以上なら必ず拾える。AI 画像メタ / EXIF / タグなど未知語だらけのコーパスに強い |

副次効果として「1 文字検索は仕様上禁止 (bigram の最小単位)」がユーザ体感と一致する。

### 6.2 なぜ DB を 3 つに分けるか

| DB | 担当 | 分離の根拠 |
| --- | --- | --- |
| `search_index.db` | Ctrl+S (フォルダ/ZIP/PDF/動画名) | 既存の手動 index 生成と互換。粒度が荒く SQLite LIKE で十分。書き込み先が SQLite 単独なので 複数 supervisor が真並列で動ける |
| `fts_index/` (Tantivy) | bigram 候補絞り込み + 検索原文 (`*_text` STORED) | Lucene 系 segment 構造。bigram 索引 + post-filter 用原文を集約することでクエリ経路から SQLite を外している |
| `fts_meta.db` (SQLite) | ファイル管理メタ (path / mtime / size / status=Ok\|Failed / index_generation) | 差分検出 (walker) の高速 IN 句 lookup、Tantivy に「どの doc が最新か」の真実の源、起動時 reconciliation で Failed 行を再 ingest 候補に集める |

検索原文を Tantivy に集約しているのは、Ctrl+G の post-filter ループから SQLite SELECT を
外して Mutex contention を消すため (INDEX_VERSION=6)。`fts_meta.db` を完全廃止しないのは、
ファイル単位の差分検出 (walker の 3-way diff) と起動時 reconciliation で SQL の集合演算を
使えると実装が単純になるから。

### 6.3 なぜ Tantivy First 書き込み順序 (§4.2) か

fts_meta.db と Tantivy の書き込みは別ストレージなので、片方だけ成功してクラッシュ
すると検索結果に古い doc が残る or 正しい doc が出ないケースが発生する。
INDEX_VERSION=6 は **Tantivy commit 成功フレームでのみ SQLite を更新する**
(Tantivy First) ことで、SQLite=Ok かつ Tantivy 未反映という状態を作らない。
中断は walker の 3-way diff (FS / Tantivy / SQLite) で「FS あり + DB なし」
として再 ingest される。Tantivy commit は fsync を伴い高コストなので、
100 件 / 5 秒のバッチ境界で commit し、その境界で SQLite を upsert / delete する。

### 6.4 なぜ Searcher snapshot を固定するか

Ctrl+G は `TopDocs::with_limit(500).and_offset(offset)` でページングするので、
ループ中に ingest worker が commit して `IndexReader` が reload すると、同じ doc が
2 ページ目に再出現したり 逆に飛ばされたりする。ワーカー開始時に 1 つの
`Searcher` を固定してループ全体を同じ snapshot で処理することでこれを回避する。
代償として「検索中に追加された doc はその検索結果に出ない」が、次の入力 /
filter 変更で再クエリされれば反映されるので実用上問題ない。

### 6.5 なぜ FavoriteEntry に UUID を持たせるか

お気に入りの path を DB キーにすると、表示名変更・ドライブ文字付替え・ZIP 内
エントリ正規化で lookup が空振りする事故が起きる。`FavoriteEntry.id: Uuid`
(serde で欠落していれば起動時に v4 発行 + settings.json 書き戻し) を Tantivy /
fts_meta.db の `favorite_id` に使うことで、表示名変更は index 保持のまま通せる。

root path 変更 (別ディレクトリへの付替え) は別扱い: 旧 path の全 doc を
Tantivy delete + SQLite delete してから新 path を再スキャンする (一括 path
更新はパス正規化 / ZIP 境界の事故が多いので採らない)。お気に入り編集
ダイアログで path 変更時に確認を出す。

### 6.6 なぜ名前索引とメタ索引を別 Supervisor にするか

| 観点 | 名前索引 | メタ索引 |
| --- | --- | --- |
| 書き込み先 | SQLite 単独 | SQLite + Tantivy (二段) |
| 処理量 / 粒度 | フォルダ単位 (数百〜数千) | 画像単位 (数万〜数十万) |
| Tantivy writer 制約 | なし | Index あたり 1 本 |
| 並列度 | 複数お気に入りで真並列 | writer Mutex を共有するので serialize |
| メンテ頻度 | upsert_children (INSERT OR REPLACE) で即反映 | バッチ commit 必須 |

性質が大きく違うので分離。Ctrl+S だけ使うユーザ (メタ索引 OFF) は軽量な
`NameIndexSupervisor` だけが常駐する。

### 6.7 なぜ `GlobalIoSemaphore` を挟むか

PDF ワーカー 3 本 / サムネイルワーカー / インデクサ (walker + ingest) が
同時に HDD をシークすると UI スクロールが 1 秒級につまる。全 I/O 経路を
優先度付き semaphore に通し、High (UI 表示中) > Normal (PDF / サムネ背景) >
Low (インデクサ) の優先順で permit を配る。

飢餓ポリシーは「UI 応答性最優先」に振っている: ユーザがアクティブに操作して
いる間 Low は止まる。アイドル時 (入力なし数秒) に Low が進む。`AC 電源時のみ
インデックス` 設定で補強。

Ctrl+G の query worker は Tantivy 読み取りなので writer 優先キューには乗らない。
ただし UI からの検索はインタラクティブ操作なので、検索中は App 側が
`ActivityGate` を継続的に bump し、背景インデクサを walker/ingest の次 checkpoint
で待たせる。これにより「入力後 1 秒でインデクサが再開し、初回ヒット表示と競合する」
揺れを抑える。

### 6.8 なぜ `try_lock + sleep` を使わないか

過去に PDF ワーカープールでこのパターンを使っていて、Critical 要求が 10 秒
ブロックされる飢餓事故が発生した。Mutex 待ち中に fresh arrival が割り込んで
Mutex を横取りし、先に待ち始めたスレッドが秒単位で待たされる。

代わりに `Mutex + Condvar で保護した優先度キュー + 専用ディスパッチャ` 構造を
使う。実装テンプレは [pdf_loader.rs](../src/pdf_loader.rs) の
`PdfWorkerPool` / `run_dispatcher`、[io_semaphore.rs](../src/io_semaphore.rs)。
詳細は [async-architecture.md §5.5](async-architecture.md)。

### 6.9 なぜタグは `dc:subject` + `#プレフィックス` か

| 選択肢 | 不採用の理由 |
| --- | --- |
| 独自 XMP 名前空間 | 他ソフト (Lightroom / Bridge / Windows Explorer) から見えない |
| Exif XPKeywords | JPEG / TIFF 限定。PNG / WebP で使えない |
| `dc:subject` に素の文字列 | 他ソフト由来のタグと mIV 独自タグが区別できず、一括削除で他ソフトのデータを壊す |
| **`dc:subject` に `#xxx` (採用)** | 業界標準プロパティで他ソフトからも見える。`#` で mIV 管理タグを識別し他ソフト由来を保護 |

副次効果: Ctrl+G で `#原神` とそのまま打てば検索できる (構文拡張不要)。

詳細は [tag-feature.md](tag-feature.md) 参照。

---

## 7. キャンセル・I/O 規約

### 7.1 キャンセルシグナル

| 対象 | 発火元 | シグナル |
| --- | --- | --- |
| Ctrl+G ワーカー | UI (クエリ変更 / フィルタ変更 / バー閉じ / folder 切替) | `GlobalSearchHandle.cancel: Arc<AtomicBool>`。Handle drop でも自動で立つ |
| Ctrl+F ワーカー | UI (同上) | `SearchPending.cancel` |
| Ctrl+S ワーカー | UI (同上) | `FavSearchPending.cancel` |
| IndexerSupervisor (全体) | `IndexerManager::sync_with_favorites` で auto_index_metadata=false に変更、または App drop | `SupervisorHandle.stop()` → cancel + watcher drop + thread join (最大 ~250ms) |
| NameIndexSupervisor | 同上 | 同構造 |
| Ingest / Walker 内部 | supervisor cancel | 各ループの checkpoint で `Ordering::Relaxed` read |
| tag_write_worker | App drop | 送信側が `None` を送る + cancel |

### 7.2 I/O セマフォの優先度マップ

| 呼び出し元 | 優先度 | permit 数 |
| --- | --- | --- |
| UI スレッドのサムネ即時ロード / FS 読み取り | High | 無制限 (semaphore は通さない。UI は専用パス) |
| PDF ワーカー (critical) / サムネイル priority ジョブ | High | 多め |
| PDF ワーカー (normal) / サムネイル通常ジョブ | Normal | 中程度 |
| 名前索引 / メタ索引 walker+ingest | Low | 速度プロファイルで 1 / 2 / 4 |

### 7.3 UI スレッドから禁止の同期 I/O

[ui-responsiveness.md §4](ui-responsiveness.md) のチェックリストを遵守する。
検索経路では特に:

- Tantivy `search()` → 必ず別スレッド (global_search::run)
- `fts_meta.db` の IN 句 SELECT → **例外的に Ctrl+F の同期経路で呼ぶ**:
  表示中 path 数が数千上限 + prepared statement なので実測 数 ms 程度
  (500 path で 9ms 程度)。数万超になったら worker 化を検討
- `notify::Watcher::new()` → supervisor spawn 時のみ
- `std::fs::read_dir` → walker スレッドのみ。UI からは呼ばない

---

## 8. テストと計装

### 8.1 統合テスト

- [tests/search_metadata_e2e.rs](../tests/search_metadata_e2e.rs) — メタ索引 E2E
  (PNG tEXt / AI メタ → ingest → Tantivy → post-filter、notify-rs での追加 / 削除 / rename 追従)
- [tests/search_name_e2e.rs](../tests/search_name_e2e.rs) — 名前索引 E2E
  (初期バルク + watcher による新規フォルダ / ZIP 追加 + 並列動作)
- [tests/common/mod.rs](../tests/common/mod.rs) — `FixtureRoot` / `start_indexer_at` /
  `wait_for_search_hits` 等のハーネス
- `src/app.rs::phase_c_key_tests` — 検索バーの相互排他 (Ctrl+F/S/G が常に ≤1 active)

進行中の Phase C (フルスタック egui_kittest ハーネス) は
[search-test-plan.md](search-test-plan.md) 参照。

### 8.2 ベンチマーク

- [docs/search-bench-results.md](search-bench-results.md) — Tantivy + bigram のプロトタイプ
  計測結果。50 万件 HARD_MAX 到達でも total 161ms、typical 10〜50ms、SQLite
  post-filter 500 件バッチ 9ms。

### 8.3 perf 計装

`--perf-log` で `perf_events.jsonl` を出力。検索 / インデクサ用の category は
`global_search` / `ingest` / `walker`。`scripts/analyze_perf.py` で解析。

---

## 9. 修正時のチェックリスト

1. **正規化の一貫性**: 新しい path lookup 経路を追加したら必ず `normalize_path`
   を通す (大文字小文字 / 区切り文字 / ZIP エントリ形式)。`normalize_for_match`
   (テキスト側) も同様。
2. **Tantivy writer は 1 本**: 新しい書き込み経路 (別 worker) を足すなら
   `IndexerManager.writer` を共有する。独自に `fts.writer()` を呼ぶと `LockBusy`
   で全 upsert が無効化される。
3. **Supervisor の drop 順序**: App drop 時は supervisor → FsWatcher → tempdir
   の順で閉じる。Windows で「使用中のファイルを削除できない」エラーを避ける。
4. **Tantivy First の書き込み順序を崩さない**: ingest の順序 (IndexDoc 構築 →
   Tantivy batch commit → SQLite upsert_meta_ok / delete_paths) は順番入替・
   削減しない。failure はキャッシュせず次回起動時の walker 3-way diff で補修
   する前提。
5. **新しい SourceKind / IndexKind を追加するなら**: `fts_index::Fields` /
   `IndexDoc` / `fts_meta::files` テーブル / `PerSourceText` / UI の
   `TargetChoice` / `KIND_CHOICES` / `search_page` のすべてに反映する。
   `INDEX_VERSION` を bump して強制再構築。
6. **UI スレッド同期 I/O の新規追加**: [ui-responsiveness.md §4](ui-responsiveness.md)
   チェックリストを通す。特に notify-rs / Tantivy / 大量 fts_meta SELECT は
   worker 化を検討。
7. **ドキュメント同時更新**: モジュール追加 → `architecture-overview.md` の
   モジュール表 / 永続化ストア表。ワーカー追加 → `async-architecture.md` の
   ワーカー表。仕様変更 → `spec.md` と `htdocs/mimageviewer/manual/`。
   この `search-architecture.md` 自体もクエリ経路 / パイプラインが変わったら更新する。

---

## 10. 関連ドキュメント

| ドキュメント | 内容 |
| --- | --- |
| [search-expansion-design.md](search-expansion-design.md) | 個別の仕様選択の詳細 (Tantivy スキーマ設計、ZIP ingest の負荷制御、UI drill-down 集約ロジック、streaming プロトコル等) |
| [tag-feature.md](tag-feature.md) | タグ機能のデータモデル (XMP dc:subject 仕様) + UI フロー + 書き込みプロトコル |
| [search-bench-results.md](search-bench-results.md) | Tantivy + bigram のプロトタイプ計測結果 (50 万件規模まで) |
| [search-test-plan.md](search-test-plan.md) | 検索 / notify-rs / キー操作の自動テスト整備計画 (Phase A/B/C) |
| [architecture-overview.md](architecture-overview.md) | リポジトリ全体のモジュールマップ・永続化ストア一覧 |
| [async-architecture.md](async-architecture.md) | ワーカー一覧・キャンセル規約・`try_lock+sleep` 禁止パターン |
| [ui-responsiveness.md](ui-responsiveness.md) | UI スレッド同期 I/O のチェックリスト (新機能追加前に §4 を必ず見る) |
