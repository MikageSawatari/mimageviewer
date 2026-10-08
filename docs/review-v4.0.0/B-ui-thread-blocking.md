# v4.0.0 コレクション 出荷前レビュー B: UI スレッドブロック / 毎フレームコスト監査

対象コミット: `0a7139d27` (master)。**すべてコードを読んだ限りの静的解析**であり、
実行・計測・再現は一切していない。数値はすべて「前提を明記した見積り」であって実測値ではない。

---

## 0. 要約

| 重要度 | 件数 |
| --- | --- |
| P1 | 1 |
| P2 | 5 |
| P3 | 12 |

**最も重い経路 3 つ** (いずれも推定):

1. **B-1 (P1)** — インポート確認モーダルが、解析済みテキストの**全行**を仮想化なしの
   `ScrollArea` へ `ui.label(format!(...))` で描く。行数上限も入力ファイルサイズ上限も無い。
   数万行の `.txt` を選ぶと、モーダルが開いている**間ずっと**毎フレーム O(行数) の
   テキストレイアウトが UI スレッドで走る。
2. **B-4 (P2)** — コレクション root での next/prev / スライドショー 1 ステップごとに、
   actor から全 entry を再 SELECT し、prepare worker が **全 entry を直列に `fs::metadata`**
   し直す (さらに動画の親フォルダを最大 64 回 `read_dir`、`VideoPinDb` を毎回 open)。
   結果が返るまで着地しない。UI スレッド自体はブロックしないが、遅い記憶域では
   ページ送りの待ちがそのまま伸び、その間 16ms 周期の repaint が回り続ける。
3. **B-2 (P2)** — コレクション root 表示中に revision が 1 進むだけで root 全体を再 install
   する。`bump_full_context_for_load` → thumbnail worker の破棄・再生成・全キャッシュ clear が
   走るため、リネームや並び順変更のように **entry 集合が変わらない編集**でも一覧の
   サムネイルが作り直しになる。§22 で navigation 経路にだけ入れた presentation identity
   ガードが、`poll_collection_grid` の install 経路には入っていない。

**構造面の総評**: actor / client / prepare worker の境界設計そのものは
`docs/ui-responsiveness.md` §2 のテンプレに忠実で、client API は `try_send` +
`Receiver` 返し、UI 側は全経路 `try_recv` になっている。**同期 I/O が UI スレッドへ
到達する経路は (既存の許容済みパターンを除いて) 見つからなかった。**
指摘の重心は「毎フレームの無駄」「大量件数時の O(N) が UI スレッド / 単一 actor に載る」
「計測手段が無い」の 3 点。

---

## 1. 指摘一覧

### B-1 [P1] インポート確認モーダルが全行を毎フレーム描画する (仮想化なし・件数上限なし)

**根拠**
- `src/ui_dialogs/collections.rs:3103-3118` — `egui::ScrollArea::vertical().max_height(320.0).show(...)`
  の中で `for line in &preview.lines { ... ui.label(format!("{}: [{status}] {}", ...)) }`。
  `show_rows` / `show_viewport` を使っていないので、可視 320px 分でなく**全行**が
  毎フレーム widget として確保・レイアウトされる。
- `src/ui_dialogs/collections.rs:3119-3126` — 同じ frame でさらに
  `preview.accepted_paths().count()` と `lines.iter().filter(...).count()` の
  O(行数) パスが 2 本追加で走る。
- `src/collection_store/text.rs:97-133` — `parse_collection_text` に行数上限が無い。
  1 行ごとに `CollectionImportLine { original: String, resolved_path: Option<PathBuf>, ... }` を
  push するだけ。
- `src/ui_dialogs/collections.rs:2884` — `std::fs::read_to_string(&worker_path)` にも
  サイズ上限が無い (worker 上なので UI はブロックしないが、巨大ファイルを選ぶと
  worker のメモリを丸ごと消費する)。

**UI スレッドまでの呼び出し連鎖**
`App::update` → `poll_collection_ui` → `poll_collection_operation`
(`CollectionDialogOperation::PreviewImport` に遷移) → 以後毎フレーム
`show_collection_manager` / `show_collection_operation_modal` →
`CollectionDialogOperation::PreviewImport` アーム (`collections.rs:3080`) →
`egui::Modal::show` closure → 上記ループ。

**想定コスト (前提を明記)**
- 前提: エクスポートした 5 万件のコレクションを再インポートする、または
  外部ツールが吐いた大きなパス一覧を読ませる。
- `ui.label` 1 回あたり `format!` (ヒープ確保) + `LayoutJob` 構築 + galley cache の
  ハッシュ引き。egui はテキストが同じなら galley を再利用するが、**LayoutJob の構築と
  ハッシュ計算・Ui の rect 確保・Id 登録は毎フレーム発生する**。
- 5 万行 × 1 行あたり 80 文字前後だと、毎フレームで数 MB 規模のハッシュ + 5 万回の
  widget 確保になる。**1 フレーム数百 ms〜秒オーダー**になり得ると推定する。
  1,000 行程度なら実用上問題ない。
- 「モーダルが開いている間ずっと」なので、ユーザーはキャンセルすら押しづらい。

**既存の同型処理がどうしているか**
- 同じファイル内の並べ替え window は `ScrollArea::show_rows` で仮想化している
  (`collections.rs:3433-3439`)。同じ画面群の中で扱いが割れている。
- 一覧側の大量件数は `docs/ui-responsiveness.md` §4.3 で
  「1 frame 最大 4,096 件に分割し、scroll では再走査しない」という規約が既にある。
- CLAUDE.md §4 チェックリストの
  「UI context 依存の大量 CPU 処理 (文字レイアウト等): 1 frame の件数上限を持つ
  state transition に分割」に該当する。

**修正方向**
1. `ScrollArea::show_rows(ui, row_h, preview.lines.len(), |ui, range| ...)` へ変更
   (行高が一定なのでそのまま使える)。
2. `accepted` / `invalid` の件数は `PreviewImport` へ遷移するときに 1 回数えて
   保持する (毎フレーム数え直さない)。
3. `parse_collection_text` 側に行数上限 (例: 100,000 = `MAX_REMOTE_COLLECTION_ENTRIES` と
   同じ根拠) と、`read_to_string` 前のファイルサイズ上限を入れ、超過は typed error で
   拒否する。プレビューは「先頭 N 行だけ表示 + 残り件数」でもよい。

---

### B-2 [P2] revision が 1 進むだけで root 全体を再 install し、thumbnail pipeline を毎回作り直す

**根拠**
- `src/app/collection_grid.rs:1282-1420` `apply_collection_grid_prepared_install` —
  既存 installed presentation との一致判定を**していない**。必ず
  `install_collection_grid_items_with_thumbnail_sources` を呼ぶ。
- `src/app/collection_grid.rs:1466-1548` — その中で
  `bump_full_context_for_load()` / `self.tx`・`self.rx` の作り直し /
  `metadata_cache.clear()` / `exif_cache.clear()` / `xmp_cache.clear()` /
  `clear_tags_cache()` / `folder_pin_map.clear()` / `reset_and_seed_auto_aspect` /
  `spawn_thumbnail_workers` / `spawn_video_thread` を実行する。
- `src/app.rs:27672-27700` `install_new_items_inner` — `thumbnails` を全件
  `ThumbnailState::Pending` に戻し、`bump_items_generation()` する。
- 一方 `src/app/collection_navigation.rs:2118-2136` では、まさにこの再 install を
  避けるために `collection_root_presentation_is_identical` +
  `thumbnail_source_identity` の一致判定を入れている (§22 の対応)。

**UI スレッドまでの呼び出し連鎖**
`App::update` → `poll_collection_grid` → (`Preparing` の `try_recv` 成功) →
`apply_collection_grid_prepared_install` → `install_collection_grid_items_with_thumbnail_sources`。

**想定コスト (前提を明記)**
- 前提: 5,000 件のコレクション root を表示中に、ツールバーから 1 件 Add する /
  コレクション名を変更する / 並び順モードを変更する。
- entry 集合が変わらない編集 (rename、同値 SetOrder、Add→Remove) でも、
  表示中のサムネイルが全部 `Pending` に戻り、可視範囲分を WebP キャッシュから
  再デコード + 再 GPU アップロードする。`docs/ui-responsiveness.md` §3.2 の実測では
  サムネ 1 枚 0.3-1ms のアップロードなので、可視 50 枚で 15-50ms + デコード分。
- さらに `prewarm_grid_tags()` (B-10) と `reset_and_seed_auto_aspect` が
  全 N 件に対して同じ frame で走る。

**既存の同型処理がどうしているか**
- 同ファイル `collection_navigation.rs:2118` が identity 一致で再 install を
  スキップする実装をすでに持っている。ガード自体は書かれていて、
  **install 経路に適用されていないだけ**。

**修正方向**
`apply_collection_grid_prepared_install` の冒頭で、`installed_presentation()` と
`(prepared, thumbnail_source_identity)` を `collection_root_presentation_is_identical`
で比較し、一致するなら `accepted_revision` / `wanted_revision` の更新と
`session.load` の差し替えだけ行って items 再 install を省く。
navigation 側と同じ述語・同じ owner を使う (述語を 2 つに増やさない)。

---

### B-3 [P2] `Snapshot` / `Preparing` 待ちに repaint 駆動が無い (推定)

**根拠**
- `src/app/collection_grid.rs:1187-1197` — `CollectionGridLoadState::Snapshot` の
  `TryRecvError::Empty` 分岐は state を戻すだけで `ctx.request_repaint*` を呼ばない。
- 同 `1245-1259` — `Preparing` の `Empty` 分岐も同じ。
- `src/app/collection_grid.rs:859-891` `open_collection_grid` /
  `907-951` `schedule_collection_grid_snapshot` にも repaint 要求が無い。
- `src/ui_dialogs/collections.rs:1799-1806` — `poll_collection_ui` の 50ms repaint 条件は
  `phase == Starting` / `catalog_request` / `snapshot_request` / `!operation.is_idle()` の
  4 つだけで、**grid session の load state は含まれていない**。
- `src/app.rs:74016-74070` — tail repaint の `reasons` 配列に collection 由来の項目が
  1 つも無い (`folder_nav_pending` / `zip_enumerate_pending` / `pdf_enumerate_pending` 等は
  ある)。
- 対照的に、同ファイルの並べ替え window (`collections.rs:1168-1170`, `1227-1229`) と
  navigation (`collection_navigation.rs:873`, `1859-1861`) は `Empty` で
  `request_repaint_after` を呼んでいる。**同じ待ちで扱いが割れている**ので、
  意図的な省略ではなく漏れと推定する。

**UI スレッドまでの呼び出し連鎖**
`open_collection_grid` (ツールバー「開く」/ メニュー) → `schedule_collection_grid_snapshot`
→ 次 frame `poll_collection_grid` が `Snapshot` を try_recv → Empty → repaint 要求なし →
`update` tail の reasons も空 (items は空、`requested` も空、`texture_backlog` も空) →
egui が就寝。

**想定コスト**
UI が止まるのではなく、**「コレクションを読み込み中…」のまま次の入力 (マウス移動等) まで
更新されない**。ホバーや tooltip の repaint で実際にはすぐ解消することが多いと思われるが、
キーボード操作だけで開いた場合や prepare が長い場合 (B-4) に固着し得る。
`docs/ui-responsiveness.md` §2 テンプレの (4) 「pending 中は `ctx.request_repaint()`」に反する。

**修正方向**
`poll_collection_grid` の `Snapshot` / `Preparing` の `Empty` 分岐、または
`update` tail の `reasons` に `collection_grid_pending` を追加する。
後者のほうが 1 か所で済み、既存の計装 (`ui.tail_repaint`) にも理由が出る。

---

### B-4 [P2] next/prev 1 ステップごとに全 entry を再分類する (直列 `fs::metadata` × N)

**根拠**
- `src/app/collection_navigation.rs:840-874` `enqueue_collection_navigation` —
  毎回 `client.subscribe()` + `client.load_collection()` を発行する。
- `src/app/collection_navigation.rs:1282-1316` `spawn_collection_navigation_prepare` —
  返ってきた snapshot に対して `prepare_collection_grid_install` を丸ごと実行。
- `src/app/collection_grid.rs:130-178` `prepare_collection_grid_install` —
  ① `prepare_collection_snapshot` (= 全 entry `inspect_collection_source`)
  ② `discover_aggregate_video_sidecars_while(..., 64, ...)`
  ③ `VideoPinDb::open_readonly(...)` **を無条件に** 実行
  ④ `prepare_collection_grid_thumbnail_sources` (= sidecar ごとに `fs::metadata` + SHA-256)。
- `src/collection_store/prepare.rs:374-400` — entry ループは**直列**。
  `rayon` 等の並列化なし。1 件ごとに `fs::metadata` 1 回。
- `src/app/folder_scan.rs:160-178` — sidecar 探索は親フォルダごとに
  `scan_directory_with_settings` (= `read_dir` 全件走査) を最大 64 回。
- `src/app/collection_navigation.rs:1319-1408` `spawn_collection_navigation_preflight` —
  さらに候補ごとに `fs::metadata` / `folder_has_still_image_with_options` /
  `scan_directory_with_convertible_archives_cancel` を実行 (`prepare.rs` とは別 worker)。

**UI スレッドまでの呼び出し連鎖**
(UI スレッドはブロックしない。待ちの連鎖として記載)
キー入力 → `start_collection_manual_navigation_inner` → `enqueue_collection_navigation`
→ actor `LoadCollection` (全 entry SELECT) → `poll_collection_navigation` →
`spawn_collection_navigation_prepare` (全 entry stat) → `spawn_collection_navigation_preflight`
→ landing。この間 `poll_collection_navigation` が毎フレーム
`ctx.request_repaint_after(16ms)` を出し続ける (`collection_navigation.rs:1859-1861`)。

**想定コスト (前提を明記)**
- 前提: 10,000 件のコレクション。`fs::metadata` がローカル SSD の warm で 5-30µs、
  SMB / 外付け / AV 常駐下で 0.5-5ms と仮定する。
- ローカル warm: 10,000 × 20µs ≒ **200ms / ページ送り**。
- ネットワーク: 10,000 × 1ms ≒ **10 秒 / ページ送り**。連打は
  `accumulate_manual` で合流するので発行数は抑えられるが、1 回の待ちは縮まない。
- そのあいだ 60Hz repaint が回り続ける (B-13)。

**既存の同型処理がどうしているか**
- 通常フォルダの `load_folder` は `read_dir` 1 回 (`ScannedDir`) を DFS スレッドで
  事前取得し、per-entry の `Path::is_dir` を排除している
  (`docs/ui-responsiveness.md` §1.1 / §5.1 対策 B')。コレクションは entry が
  任意パスなので `read_dir` に畳めないケースはあるが、**同一親のものは畳める**。
- スマートフォルダ / 詳細遅延列は「対象集合 revision が変わらない限り再走査しない」
  (`docs/ui-responsiveness.md` §4.3)。コレクションの navigation はこれを持たず、
  revision 不変でも毎回フルスキャンする。

**修正方向**
1. **revision が変わっていないときは prepare を省略する**。navigation は既に
   `session.accepted_revision` と installed presentation を持っているので
   (`collection_navigation.rs:2118`)、そこへ入る前に「actor load すら不要」の
   ファストパスを作る。現状は「actor load → prepare → 一致したので再 install しない」と、
   一番高い部分を実行してから捨てている。
2. prepare の entry ループを `rayon` の `par_iter` にする (`fs::metadata` は I/O 待ちが
   支配的なので効果が大きい)。cancel チェックは維持する。
3. 同一親の entry は親ごとに `read_dir` 1 回へ畳み、`DirEntry::file_type()` /
   `DirEntry::metadata()` を使う (CLAUDE.md の `read_dir` 規約と同じ理由)。
4. `VideoPinDb::open_readonly` は `videos.is_empty()` のとき呼ばない
   (`collection_grid.rs:161-165`、現状は動画 0 件でも毎回 open している)。

---

### B-5 [P2] `migrate_source_batch` が全コレクションの全 entry を読み、M×N の突き合わせを単一 actor 上で行う

**根拠**
- `src/collection_store/db.rs:362-398` — `load_all_entries(&tx)` で**全コレクションの
  全 entry** をメモリへ読み、`for entry in &entries { for migration in &batch.migrations { ... } }`
  の二重ループで `replacement_for` を呼ぶ。さらに `resulting_keys` に全 entry の
  key を clone して HashSet を作る。
- `src/app.rs:31628-31640` `enqueue_collection_source_migration_batch` →
  `try_start_next_rename_migration` → worker → actor `MigrateSourceBatch`。
- `src/app.rs:34563-34573` — 本のページ移動・並べ替えの完了ごとに
  `book_collection_source_mappings` が **ページ数分の Exact mapping** を作って投げる。

**UI スレッドまでの呼び出し連鎖**
(UI はブロックしない。単一 actor の占有として記載)
リネーム / 移動 / 本のページ転送の成功 → `enqueue_collection_source_migration_batch` →
rename migration worker → actor `MigrateSourceBatch` → 上記二重ループ。
この間 actor は他の `ListCatalog` / `LoadCollection` を一切処理できず、
UI の `schedule_collection_grid_snapshot` / `request_catalog` と Remote の要求が
すべて後ろに並ぶ。

**想定コスト (前提を明記)**
- 前提: 全コレクション合計 50,000 entry、本のページ転送で 500 mapping。
- 500 × 50,000 = 25,000,000 回の `replacement_for` (path 正規化 + prefix 比較)。
  1 回 100ns でも 2.5 秒、文字列確保を伴えば桁が上がる。
- さらに 50,000 回の `source_key.clone()` + HashSet 挿入。

**既存の同型処理がどうしているか**
- 他の path-keyed ストアの移行は `rename_key_migration` worker が DB 側で
  prefix UPDATE を発行する形 (`docs/ui-responsiveness.md` §2.4 の journal writer)。
  コレクションだけが「全件をアプリ側へ読んで突き合わせる」形になっている。

**修正方向**
- mapping 側を正規化キーの `HashMap` / prefix 用のソート済み配列にして、
  entry ごとの走査を O(log M) か O(1) にする (現状は entry ごとに M 本を線形に試す)。
- `load_all_entries` を `normalized_path` の exact / prefix 条件で SQL 側に絞る。
  `UNIQUE(collection_id, source_namespace, normalized_path)` が既にあるので
  Exact は index で引ける。Tree は `normalized_path >= ? AND normalized_path < ?` の
  範囲条件に落とせる。
- `resulting_keys` の全件重複検査は、実際に置換される行 + その collection の
  既存 key だけに限定する。

---

### B-6 [P2] コレクション機能に `perf::event` 計装が 1 つも無い

**根拠**
- `grep -rn "perf::event\|perf::is_enabled" src/collection_store/ src/app/collection_grid.rs
  src/app/collection_navigation.rs src/ui_dialogs/collections.rs
  src/remote_ipc/persistent_collections.rs` → **ヒット 0**。
- `src/app.rs:74016-74070` の tail repaint reasons にも collection が無い (B-3)。

**なぜ問題か**
- CLAUDE.md「レビュー時: UI 応答性の性能観点」と `docs/ui-responsiveness.md` §4 の
  チェックリストが「追加した同期処理の区間には perf::event を必ず差し込む
  (悪化を検知できるように)」を必須としている。
- 上の B-1 / B-2 / B-4 / B-5 はいずれも **`--perf-log` からは一切見えない**。
  リリース手順 Phase 2 の `perf_smoke.ps1` / `check-idle-health.ps1` を回しても、
  コレクション由来のヒッチを原因区間へ結び付けられない。
- 特に `check-idle-health.ps1` は「同一 thumbnail work の反復」と
  「repaint reason streak」を見るが、collection の repaint 理由が
  `tail_repaint reasons` に出ないため、B-3 / B-13 由来の streak が
  `causes` 側の file:line からしか追えない。

**修正方向**
最低限、次の 5 区間に `cat="collection"` のイベントを入れる
(`if crate::perf::is_enabled()` の外ガード付き):
- `kind="actor_rtt"` — `load_collection` / `list_catalog` の enqueue→try_recv 成功まで
  (`collection_id`, `entries`, `ms`)
- `kind="prepare"` — `prepare_collection_grid_install` の
  `stage=classify / sidecar_scan / pin_db / identity` 別 `ms` と `entries` / `parents`
- `kind="preflight"` — `preflight_candidates` の `candidates` / `ms` / `outcome`
- `kind="install"` — `apply_collection_grid_prepared_install` の `ms` / `entries` /
  `reused`(identity 一致で再 install を省いたか)
- `kind="migrate"` — actor `MigrateSourceBatch` の `entries` / `mappings` / `ms`

`docs/ui-responsiveness.md` にも節を追加し、`scripts/analyze_perf.py` に
サブコマンドを足せる形にしておく。

---

### B-7 [P3] `reconcile_collection_toolbar_target` が毎フレーム HashSet と Vec を作り直す / `settings.save()` へ到達し得る

**根拠**
- `src/ui_dialogs/collections.rs:1513-1550` — `poll_collection_ui` から毎フレーム呼ばれ、
  catalog 全 definition の `HashSet<Uuid>` 構築、`seen` HashSet 構築、
  `self.settings.pinned_collections.clone()` (Vec clone)、`retain`、
  definitions の線形検索を実行する。catalog revision が変わっていなくても毎回。
- `src/ui_dialogs/collections.rs:1797` — `poll_collection_ui` からの呼び出し。
- `src/ui_dialogs/collections.rs:1345-1351` `persist_collection_toolbar_target` →
  `self.settings.save()` (同期 SQLite 書き込み)。
- `src/app.rs:74497` — `poll_collection_ui` は `App::update` の**最上部**、
  fullscreen / native video の early return **より前**で呼ばれる。

**想定コスト**
- コレクション 20 件で HashSet 2 個 + Vec clone ≒ 数 µs/frame。**絶対値は小さい**。
  ただし CLAUDE.md の「走査の形が既存と同じでも係数は別。毎フレーム経路へ足すなら
  同条件で測ってから報告する」に照らすと、idempotent な reconcile を毎フレーム
  回す理由は無い。
- `settings.save()` 側は読んだ限り冪等 (prune 後は `pins_changed` が false、
  target も固定点に収束する) なので**毎フレーム保存にはならない**と判断した。
  ただし発火したときは `docs/ui-responsiveness.md` §2.5 の実測で
  中央値 2.1ms / 最大 6.2ms / 初回 8.3ms の同期 SQLite 書き込みが、
  **動画再生中の frame でも** 走り得る (early return より前のため)。

**修正方向**
`session`/`collection_ui` に `reconciled_catalog_revision: u64` を持たせ、
`catalog.catalog_revision` と `settings.pinned_collections` の世代が変わったときだけ
実行する。`settings.save()` は既存の例外扱い (§2.5) のままでよいが、
呼ぶ位置を early return より後ろの通常 frame 経路へ寄せられるなら寄せる。

---

### B-8 [P3] `collection_toolbar_catalog()` が毎フレーム全名前を clone する (セクション非表示でも)

**根拠**
- `src/ui_dialogs/collections.rs:1416-1432` — `definitions.iter().map(|d| (d.id, d.name.clone())).collect()`
  で毎回新しい `Vec<(CollectionId, String)>` を作る。
- `src/ui_main.rs:8256-8264` — `render_toolbar` の冒頭で無条件に呼ぶ。
  `show_collections` が false でも、`any_toolbar_section` の early return
  (`ui_main.rs:8293-8295`) **より前**にある。`toolbar_pinned_collections` の
  `Vec<CollectionId>` 構築も同様。
- `src/ui_main.rs:6085-6095` — 上部メニューでも `menu_button` closure の**外**で
  呼んでおり、メニューを開いていないフレームでも実行される。
  `collection_definition()` (`collections.rs:1246-1257`) も同じ位置で
  `CollectionDefinition` を丸ごと clone する。
- `src/app.rs:73255` — `render_toolbar` はメインウィンドウの毎フレーム呼び出し。

**想定コスト**
コレクション 50 件で 50 回の `String` 確保 × 2 か所 = 100 alloc/frame、60fps で 6,000/s。
mimalloc 下なら数 µs。**小さいが、表示していない機能のための純粋な無駄**。

**既存の同型処理がどうしているか**
本棚は `self.book_list_cache.clone()` (`ui_main.rs:8269`) で `Option<Vec<..>>` を
clone しており、同じ癖がある。ただし本棚は `show_bookshelf` の判定と同じ位置で
`request_book_list_refresh()` をガードしている。

**修正方向**
`collection_toolbar_catalog()` を `&self` から借用を返す形 (`&[CollectionDefinition]` +
target id) に変え、描画側で必要なときだけ `name` を借りる。
最低限、`show_collections` / メニュー open のガードの内側へ移す。

---

### B-9 [P3] 再 install 時の selection / checked / search filter の remap が O(件数 × entry 数)

**根拠**
- `src/app/collection_grid.rs:1369-1378` — `old_checked` の各 anchor について
  `prepared.entries.iter().position(|e| e.entry_id == anchor.entry_id)` を実行。
- 同 `1345-1368` / `1391-1408` — `old_search_anchors` も同様に `position()`。
  さらに entry_id で見つからなければ `source_key` でもう一度全走査する。

**UI スレッドまでの呼び出し連鎖**
`App::update` → `poll_collection_grid` → `apply_collection_grid_prepared_install`。

**想定コスト (前提を明記)**
- 前提: 10,000 件のコレクションで Ctrl+A 相当の全チェック後に revision が進む。
- 10,000 × 10,000 = 10^8 回の Uuid 比較。**数百 ms〜秒**の UI ブロックになり得る。
- 通常の使い方 (数件チェック) では無視できる。

**修正方向**
`prepared.entries` から `HashMap<CollectionEntryId, usize>` と
`HashMap<&CollectionSourcePathKey, usize>` を 1 回作り、以後は O(1) で引く。

---

### B-10 [P3] install 時に `prewarm_grid_tags()` が UI スレッドで全件 SQLite SELECT を行う

**根拠**
- `src/app/collection_grid.rs:1512` — `install_collection_grid_items_with_thumbnail_sources`
  から呼ぶ。
- `src/app.rs:53666-53692` — 全 items の tag key を作ってソート・dedup し、
  `db.get_many_display_tags(&keys)` を同期実行する。

**UI スレッドまでの呼び出し連鎖**
`App::update` → `poll_collection_grid` → `apply_collection_grid_prepared_install` →
`install_collection_grid_items_with_thumbnail_sources` → `prewarm_grid_tags`。

**既存の同型処理がどうしているか**
通常の `load_folder` (`app.rs:25169`)、ファイル名スタック (`filename_stack_ui.rs:793`)、
全文検索 (`global_search_ui.rs:1341`) も同じ場所で同じ関数を呼ぶ。
**したがってコレクション固有の新規退行ではない。**

**なぜ挙げるか**
コレクションは通常フォルダより件数が大きくなり得るうえ、B-2 のとおり
**revision が動くたびに再実行される** (フォルダ切替は 1 回きり)。
B-2 を直せば発火頻度が落ちるので、B-2 の副次効果として扱えばよい。

**修正方向**
B-2 を直す。それでも遅いなら `docs/ui-responsiveness.md` §4.3 と同じく
1 frame あたりの件数上限を持つ分割にする。

---

### B-11 [P3] `collection_root_presentation_is_identical` が UI スレッドで全 entry の deep 比較を行う

**根拠**
- `src/app/collection_navigation.rs:213-218` — 実体は `installed == prepared`。
- `src/collection_store/prepare.rs:55-73` — `CollectionPreparedSnapshot` /
  `PreparedCollectionEntry` は `#[derive(PartialEq)]`。`Arc<[T]>` の `==` は
  **ポインタ比較ではなく内容比較**。1 entry あたり `CollectionSourcePathKey` (文字列)、
  `PathBuf`、`availability` (`AccessError(String)` を含む)、`GridItem` (PathBuf を含む) を
  比較する。
- `src/app/collection_navigation.rs:2130-2135` — navigation landing で毎回呼ぶ。

**想定コスト (前提を明記)**
- 前提: 10,000 件、1 entry あたり path 100 文字前後。
- 1 回のページ送りあたり、およそ 3MB 分の memcmp + 10,000 回の enum 判別。
  **0.3-1ms 程度**と推定。10 万件なら 10ms 級。
- B-4 の 200ms〜秒オーダーに比べれば小さいが、こちらは **UI スレッド上**。

**修正方向**
`prepare` worker が既に `CollectionGridThumbnailSourceIdentity` (SHA-256) を
作っているので、同じ worker で `entries` 側の固定長ダイジェストも作って
32 バイト比較 1 回に落とす。§22 が言う「一つの presentation identity を所有する」
という設計意図とも合う (現状は sidecar/pin だけがダイジェスト化されている)。

---

### B-12 [P3] `add_batch` が 1 行あたり 2 本の未キャッシュ SQL を発行し、その間 actor を占有する

**根拠**
- `src/collection_store/db.rs:196-222` — `for registration in registrations` の中で
  `source_exists(&tx, ...)` (`query_row`) と `tx.execute("INSERT ...")` を呼ぶ。
  どちらも `prepare_cached` を使っていないので毎回 statement を prepare する。
- `src/collection_store/runtime.rs:859-869` — actor は単一スレッドの
  `select_biased!` ループ。`process_command` の実行中は他要求を一切拾えない。

**想定コスト (前提を明記)**
- 前提: 5 万行のテキストをインポート。
- 1 行あたり prepare×2 + 実行×2 で 50-100µs と仮定すると **2.5-5 秒** 1 トランザクション。
- その間 UI の `LoadCollection` / `ListCatalog` と Remote 要求が全部待つ
  (UI は `try_recv` なのでフリーズはせず、「読み込み中」が伸びるだけ)。

**修正方向**
`tx.prepare_cached(...)` で 2 本の statement を使い回す。
インデックスは `UNIQUE(collection_id, source_namespace, normalized_path)`
(`db.rs:479`) があるので追加不要。

---

### B-13 [P3] navigation pending 中の repaint が 16ms 周期 (他は 50ms)

**根拠**
- `src/app/collection_navigation.rs:873` / `1859-1861` —
  `ctx.request_repaint_after(Duration::from_millis(16))`。
- `src/ui_dialogs/collections.rs:1170` / `1230` / `1805` — 同じ「worker の結果待ち」で 50ms。

**想定コスト**
B-4 のとおり待ちが数秒に伸びる状況では、その間 60Hz で `App::update` が回る。
`docs/idle-health-check.md` の update rate / repaint reason streak の観点では、
操作中なので idle 判定には入らないはずだが、ノート PC の電力と
`perf_smoke` のフレーム分布には効く。

**修正方向**
50ms へ揃えるか、actor/worker の完了を wake channel で拾って
`request_repaint()` を 1 回だけ出す形にする
(`CollectionRevisionWatch::wake_receiver()` が既にあるので、同種の wake を
prepare/preflight worker にも持たせられる)。

---

### B-14 [P3] prepare / navigation spawn のたびに `Settings` 全体を clone する

**根拠**
- `src/app/collection_grid.rs:960-961` — `let settings = self.settings.clone();`
- `src/app/collection_navigation.rs:1291-1292` — 同じ。

**想定コスト**
`Settings` は favorites / keymap / toolbar 順 / smart folders 等の `Vec<String>` を
多数持つ。1 回あたり数十〜数百の確保。B-4 のとおり**ページ送りごとに**発生する。

**修正方向**
prepare が実際に使うのは `grid_display_order` と、
`discover_aggregate_video_sidecars_while` が見る
`skip_image_if_video_exists` / `video_thumb_use_sidecar_image` と
`scan_directory_with_settings` の入力だけ。必要フィールドだけの
`Arc<CollectionPrepareSettings>` を作って渡す。

---

### B-15 [P3] 並べ替え window / 管理 window の毎フレーム clone

**根拠**
- `src/ui_dialogs/collections.rs:3609` — 可視セルごとに
  `let path = entry.entry.source_path.clone();` (hover closure 用)。
  `show_rows` で仮想化はされているので可視分のみ。
- 同 `3535-3540` — セルごとに `format!("{:04}", index + 1)`。
- `src/ui_dialogs/collections.rs:2557` — `let catalog = self.collection_ui.catalog.clone();`
  管理 window が開いている間、毎フレーム catalog 全体を deep clone する。
- 同 `2646-2653` — ScrollArea 内で毎フレーム `HashSet<CollectionId>` を構築し
  `manager_rename_inputs.retain(...)` する。

**想定コスト**
可視 100 セル × (PathBuf clone + format!) = 200 alloc/frame。
管理 window は collection 数ぶんの String clone/frame。いずれも小さい。

**修正方向**
hover テキストは `on_hover_ui` ではなく `on_hover_text_at_pointer` に
`&entry.entry.source_path` を借用で渡せる形にするか、hover したセルだけ
clone する。管理 window の `retain` は catalog revision 変化時だけにする。

---

### B-16 [P3] revision watch の subscriber がナビゲーションごとに増え、publish 時にしか掃除されない

**根拠**
- `src/app/collection_navigation.rs:860` — `enqueue_collection_navigation` が
  **要求ごとに** `client.subscribe()` を呼ぶ。
- `src/collection_store/runtime.rs:445-471` `subscribe` — `hub.subscribers` へ
  `Weak` を push するだけ。
- `src/collection_store/runtime.rs:1026-1048` `publish_revision` — 死んだ Weak は
  ここでしか `retain` されない。

**想定コスト**
コレクションを編集せずに 10,000 回ページ送りすると、`hub.subscribers` に
10,000 個の死んだ `Weak` が溜まる (16 バイト × 10,000 = 160KB 程度)。
次の publish で一括 prune されるので恒久リークではない。UI スレッドの
`subscribe()` は `Vec::push` 1 回なので O(1)。

**修正方向**
`subscribe()` 側でも `subscribers.len()` が閾値を超えたら prune する、または
navigation が session の watch を使い回す (grid session は既に
`session.watch` を持ち回している。navigation だけが毎回作っている)。

---

### B-17 [P3] `VideoPinDb::open_readonly` を動画 0 件でも毎回開く

**根拠**
- `src/app/collection_grid.rs:161-165` — `videos` が空でも
  `VideoPinDb::open_readonly(&VideoPinDb::db_path())` を呼び、
  `lookup_webps_many(videos.iter())` に空イテレータを渡す。
- 直前の `discover_aggregate_video_sidecars_while` は
  `requested.is_empty()` で早期 return する (`folder_scan.rs:137-139`) ので、
  対比として不揃い。

**想定コスト**
SQLite の cold open は `docs/ui-responsiveness.md` §1 で「cold 30-150ms」。
worker 上なので UI はブロックしないが、B-4 のとおりページ送りごとに発生する。

**修正方向**
`if videos.is_empty() { Default::default() } else { ... }` を足す。

---

### B-18 [P3] `inspect_collection_source` が entry ごとに `fs::metadata` を撃つ (同一親でも畳まない)

**根拠**
- `src/collection_store/prepare.rs:558-566` — 1 entry = 1 `fs::metadata`。
- CLAUDE.md /`docs/ui-responsiveness.md` §1.1: Windows では per-entry の
  `GetFileAttributes` が数百エントリで数百 ms になり、`read_dir` +
  `DirEntry::file_type()` に畳むのが規約。

**なぜ「同型処理と違う」か**
コレクションの entry は任意パスなので原理的に `read_dir` へ畳めないケースはある。
ただし実用上は「あるフォルダの中身をまとめて追加した」コレクションが多いはずで、
その場合は親ごとに 1 回の `read_dir` で全件の種別・サイズ・mtime が取れる。

**修正方向**
entry を親ディレクトリでグループ化し、同一親に 2 件以上あるときだけ
`read_dir` を 1 回まわして `DirEntry` から facts を取る。単独 entry は
従来どおり `fs::metadata`。B-4 の並列化と組み合わせると効果が大きい。

---

### B-19 [P3] `on_exit` の migration drain が無期限 `recv()`

**根拠**
- `src/app.rs:31525-31533` — `match rx.recv()`。タイムアウト無し。
- `src/app.rs:74558` — `eframe::App::on_exit` から呼ぶ。
- 対になる actor 側は `Command::reject()` (`runtime.rs:872-874`) で必ず reply を返すか
  sender を drop するので、**通常は必ず終わる**。

**想定コスト**
B-5 のとおり `MigrateSourceBatch` が数秒かかる構成では、その分だけ終了が遅れる。
actor が長い transaction の途中だと、`Shutdown` は `select_biased!` の
次のループまで届かないので、最悪 `MigrateSourceBatch` 1 本ぶん待つ。

**既存の同型処理がどうしているか**
`CollectionRemoteProducerControl::close_and_drain` (`runtime.rs:289-311`) も
`Condvar::wait` の無期限待ちだが、こちらは `lib.rs:1511` の
`run_native` 復帰後 = プロセス最終で、plan §4.2 の許容境界そのもの。
`resolve_collection_migration_for_exit` は `on_exit` (まだ eframe 内) なので
一段手前だが、plan §4.2 の「App final exit の drain だけが待機可能」に収まる。

**修正方向**
仕様上は許容範囲。B-5 を直せば実質的な待ちは消える。
念のため `recv_timeout` + タイムアウト時は boot-retry へ積む形にすると、
actor 異常時にプロセスが閉じないケースを防げる。

---

### B-20 [P3] `phase == Starting` の 20Hz repaint に上限が無い

**根拠**
- `src/ui_dialogs/collections.rs:1799-1806` — `Starting` の間
  `request_repaint_after(50ms)` を出し続ける。
- `src/collection_store/runtime.rs:823-838` — actor は open 失敗時に必ず
  `Failed` を送るので、通常は数 ms〜(busy_timeout の) 3 秒で終わる。

**想定コスト**
DB がネットワーク上にある / ロックされている等で `open_at` が長引くと、
その間 20Hz の `App::update` が回る。`check-idle-health.ps1` の
static-foreground / static-background は「測定開始前に準備完了」を前提に
するので通常は引っかからないが、起動直後を含む窓では update rate に効く。

**修正方向**
現状のままでも実害は小さい。気になるなら `Starting` の経過時間で
repaint 間隔をバックオフする (50ms → 200ms → 1s)。

---

### B-21 [P3] Remote と PC UI が単一 actor を共有する

**根拠**
- `src/remote_ipc/pipe.rs:763-765` — `PersistentCollectionEngine::new(producer)`。
- `src/collection_store/runtime.rs:231-233` — producer は
  `CollectionStoreClient` を包むだけで、同じ actor へ送る。
- `src/remote_ipc/persistent_collections.rs:42` — `MAX_REMOTE_COLLECTION_ENTRIES = 100_000`。

**想定コスト**
Remote が 10 万件のコレクションの `LoadCollection` を投げると、その SELECT の間
PC 側の `schedule_collection_grid_snapshot` / `request_catalog` は待つ。
UI は `try_recv` なのでフリーズはせず、表示が遅れるだけ。
`COMMAND_CAPACITY = 128` を超えると UI 側は `Busy` を受け取り、
`schedule_collection_grid_snapshot` は `Failed { message: "コレクション処理が混み合っています" }` に
落ちる (`collection_grid.rs:942-948`)。この Failed は
「terminal failureは毎frame再requestせず、明示reload/reopen/新しいnoticeだけがretry owner」
(plan §16.1) なので、**Remote 由来の一時的な Busy で PC 側の一覧が
手動リロードするまで止まる**可能性がある (推定)。

**修正方向**
`Busy` は terminal ではなく再試行可能として扱い、
`RequestNeeded` へ戻して次 frame (または短い backoff 後) に再送する。
CLAUDE.md の「資源が競合したら利用者が待っている前面を通す」に従うなら、
Remote 側の要求を別レーン (低優先度) にするか、read 専用の 2 本目の
SQLite read connection を actor に持たせる。

---

## 2. 監査したが「UI スレッドへ同期到達しない」と判断した候補 (再監査防止)

各行 = grep ヒットと判定理由。行番号はコミット `0a7139d27` 時点。

- `src/collection_store/db.rs:27,30,40,43` `try_exists` / `Connection::open_with_flags` /
  `create_dir_all` / `Connection::open` — `actor_main` (`runtime.rs:815`) 先頭のみ。
  actor は `std::thread::Builder::spawn` (`runtime.rs:104`) 上。UI 到達なし。
  `busy_timeout(3s)` を `journal_mode` 変更より**前**に設定済み (メモリの SQLite WAL 注意点に適合)。
- `src/collection_store/prepare.rs:559` `fs::metadata` (`inspect_collection_source`) —
  `collection-grid-prepare` / `collection-navigation-prepare` / remote worker からのみ。UI 到達なし (B-4/B-18 で別途指摘)。
- `src/collection_store/prepare.rs:540-547` `OpenOptions::open` / `write_all` / `flush` / `sync_all` —
  `collection-export` WorkerTask (`collections.rs:2496`) のみ。UI 到達なし。
- `src/collection_store/prepare.rs:628` `fs::remove_file` (`TempExportFile::drop`) — 同 worker 内。UI 到達なし。
- `src/collection_store/path.rs` 全体 — ファイルシステムアクセスなし (純粋な lexical 正規化)。
- `src/collection_store/text.rs` 全体 — `parse_collection_text` / `serialize_collection_paths` とも純関数。plan §8.1 の「preview までは対象へアクセスしない」を満たす。
- `src/collection_store/runtime.rs:66,79,168,262,279,301,329,345,449,461,669,691,841,1028,1044,1052` `Mutex::lock` —
  すべて短時間保持 (フィールド読み書きのみ)。`publish_revision` (1026) だけ subscriber の Vec を retain するが、
  clone 済み Vec を作ってからロック外で通知しており、ロック区間は O(subscriber 数)。
- `src/collection_store/runtime.rs:186,189` `ack.recv()` / `join.join()` — `shutdown_and_join`。
  呼び出しは `lib.rs:1514` (`run_native` 復帰後) と `collections.rs:570` (`shutdown_for_exit`)。
  通常起動では runtime の所有は process 側 (`install_process_owned_collection_runtime`) なので
  `self.runtime` は None で join しない。plan §4.2 の許容境界。
- `src/collection_store/runtime.rs:310` `Condvar::wait` (`close_and_drain`) — `lib.rs:1511`、
  `run_native` 復帰後のプロセス最終のみ。
- `src/collection_store/runtime.rs:436-438` `Drop for CollectionStoreRuntime` —
  `JoinHandle::is_finished()` が真のときだけ join。通常 Drop は待たない。
- `src/collection_store/runtime.rs:452-470,689-704` `CollectionStoreClient::request` /
  `subscribe` — `try_send` + `Receiver` 返し。満杯は `Busy`、Closing 以降は `Unavailable`。
  **UI が one-shot receiver を `recv()` でブロックする経路は見つからなかった。**
- `src/app/collection_grid.rs:90` `fs::metadata` (thumbnail identity) —
  `prepare_collection_grid_thumbnail_sources` は `prepare_collection_grid_install` 経由でのみ呼ばれ、
  その呼び出し元は `spawn_collection_grid_prepare` (`collection_grid.rs:965`) と
  `spawn_collection_navigation_prepare` (`collection_navigation.rs:1296`) の 2 つの
  `std::thread::Builder::spawn` のみ。UI 到達なし。
- `src/app/collection_grid.rs:141` `discover_aggregate_video_sidecars_while` (内部で `read_dir` ×最大64) — 同上 worker。
  `scan_directory_with_settings` は既存の `DirEntry::file_type()` 経路なので §1.1 規約に適合。
- `src/app/collection_grid.rs:161` `VideoPinDb::open_readonly` — 同上 worker (B-17 で頻度のみ指摘)。
- `src/app/collection_navigation.rs:519` `fs::metadata` / 526-536 `folder_has_still_image_with_options` /
  `scan_directory_with_convertible_archives_cancel` — `preflight_candidates` は
  `spawn_collection_navigation_preflight` の worker (`collection_navigation.rs:1400`) からのみ。UI 到達なし。
- `src/app/collection_navigation.rs:1373` `self.pdf_open_password(&target.source_path)` —
  `app.rs:25436` は in-memory HashMap 参照。DPAPI / SQLite へ行かない。
- `src/ui_dialogs/collections.rs:2884` `fs::read_to_string` — `collection-import-read` WorkerTask。UI 到達なし
  (サイズ上限が無い点は B-1 に含めた)。
- `src/ui_dialogs/collections.rs:125,149` `JoinHandle::join` — `poll_finished` / `Drop` とも
  `is_finished()` が真のときだけ join。ブロックしない。
- `src/ui_main.rs:6131,6149` `rfd::FileDialog::pick_file/save_file` — メニュークリックのハンドラ内。
  UI スレッドのネイティブモーダルだが、`docs/ui-responsiveness.md` §4.2 と同じく
  **ユーザーが明示的に開始したモーダル操作**であり、既存の他ダイアログと同じ扱い。指摘しない。
- `src/remote_ipc/persistent_collections.rs:557,716,968,1088` `inspect_collection_source` /
  `prepare_collection_snapshot_while` — `PersistentCollectionEngine` は
  `pipe.rs:764` で構築され、`home_worker_loop` (`pipe.rs:780-790`) と
  `worker_loop` (`pipe.rs:991`) のワーカースレッドからのみ呼ばれる。**UI スレッドではない。**
- `src/remote_ipc/persistent_collections.rs:751-768` `wait_actor_reply` —
  `select_biased!` + `default(remaining)` (最大 `EXACT_REQUEST_BUDGET` = 9s) + session/producer wake。
  ワーカースレッド上で、無期限ブロックにならない。`sleep` / busy loop なし
  (`awk 'NR<1632' | grep -E "sleep|loop \{|while "` のヒットは `persistent_collections.rs:287` の
  レスポンス縮小ループのみで、毎周回で必ず要素を減らす有限ループ)。
- `src/app/top_level_grid_view.rs` 全体 — ファイル I/O / DB / texture なし。
  `collection_session()` / `generation()` / `surface()` はすべて O(1)。
- `src/rename_key_migration.rs` — collection 側の呼び出しは
  `PathMigrationJob::collection_only` (73) の構築だけ。`app.rs:31628` の enqueue は
  journal writer (`docs/ui-responsiveness.md` §2.4 の latest-value writer) 経由で非同期。
  `app.rs:11113` `book_collection_source_mappings` は純粋なマッピング構築 (I/O なし)。
- `src/app/collection_grid.rs:1806-3260` / `src/app/collection_navigation.rs:2872-4300` /
  `src/ui_dialogs/collections.rs:3797-6109` / `src/remote_ipc/persistent_collections.rs:1632-1940` —
  すべて `#[cfg(test)]` 配下。`fs::write` / `thread::sleep` / `recv_timeout` のヒットは
  テストフィクスチャで、製品経路ではない。
- `GridItem::CollectionPlaceholder` — `app.rs:27694` で初期 `ThumbnailState::Failed` に
  されるため、`Pending` として prefetch 抑制や毎フレーム repaint を誘発しない。**適切**。
- `collection_reorder_textures_for` (`collections.rs:965-1005`) — 既存 grid の
  `TextureHandle` を clone するだけで `ctx.load_texture` を新規に呼ばない。
  **並べ替え window / hover preview で GPU アップロードは発生しない。** 適切。
- `collection_grid_content_target` (`collection_grid.rs:229-260`) /
  `collection_grid_stamp` (200-209) / `collection_root_delete_resolution` (369-430) —
  いずれも O(1) または O(checked)。毎フレーム経路 (`ui_main.rs:6087`) でも問題なし。
- `poll_collection_grid` / `poll_collection_navigation` のコレクション非表示時 —
  `collection_session()` が None で即 return。非コレクション表示中の毎フレームコストはほぼゼロ。**適切**。

---

## 3. 既存規約との整合まとめ

| 規約 | 判定 |
| --- | --- |
| `read_dir` ループで `DirEntry::file_type()` を使う (CLAUDE.md / §1.1) | 適合 (既存 `scan_directory_with_settings` を再利用)。ただし B-18 のとおり、そもそも `read_dir` に畳めるところで per-path `fs::metadata` を撃っている |
| `try_lock` + sleep を使わない (CLAUDE.md) | 適合。sleep / try_lock 再試行は製品経路に無い |
| actor は bounded nonblocking enqueue + one-shot response (plan §4.1) | 適合。`try_send` / `try_recv` のみ |
| 通常 frame / dialog close / viewer close で join しない (plan §4.2) | 適合 (B-19 の `on_exit` だけが待機、これは許容境界) |
| prepare worker が filesystem を所有し UI は stat しない (plan §6.1) | 適合。UI スレッドからの `stat` / canonicalize / folder scan は見つからなかった |
| preview 段階で対象パスへ I/O しない (plan §8.1) | 適合 (`parse_collection_text` は純関数) |
| export は worker が書く (plan §8.3) | 適合 |
| cancel token の伝搬 (§2.2 の 3 箇所) | 適合。新規起動時 (`cancel_pending`)、context 終了時 (`CollectionGridSession::Drop`)、ループ内 (`prepare.rs:385`, `collection_grid.rs:80` 等) すべて確認 |
| SQLite cold open の回数 | B-17 (動画 0 件でも VideoPinDb を open) 以外は問題なし |
| **`perf::event` 計装 (§4 チェックリスト)** | **未適合 — B-6** |
| pending 中の `ctx.request_repaint()` (§2 テンプレ (4)) | **一部未適合 — B-3** |

---

## 4. 出荷判断の目安 (レビュアー私見)

- **B-1 は出荷前に直すべき**と考える。ユーザーが自分で作ったエクスポートを
  そのまま読み戻す動線で踏み得るうえ、直し方が `show_rows` への置換 + 件数上限で
  局所的に済む。
- **B-2 / B-3 は小さい修正で効果が大きい**。B-2 は既にある述語を install 経路にも
  適用するだけ、B-3 は tail repaint の reasons に 1 行足すだけ。
- **B-4 / B-5 は構造的で、出荷を止めるかは件数の想定次第**。
  「コレクションは数百件まで」という前提で出すなら次版送りでよいが、
  その前提を製品ドキュメントか backlog のどこかに明記しておかないと、
  後から「大きなコレクションで遅い」という報告が来たときに
  設計判断だったのか退行なのかを区別できなくなる。
- **B-6 (計装なし) は、上の判断を実測で検証できないという意味で効く。**
  B-4 / B-5 を次版送りにするなら、せめて計装だけは先に入れて
  `perf_smoke` に現れるようにしておきたい。
