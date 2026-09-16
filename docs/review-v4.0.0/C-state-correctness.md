# v4.0.0 コレクション 出荷前レビュー — 担当C「状態所有・ライフサイクル・正しさ」

対象: `d8b61ff20^..HEAD`。**読み取り専用**。ビルド・テスト・アプリ起動は行っていない。
以下はすべて**コードを読んだ限りの推定**であり、実機での観測ではない。実行時の挙動について
私自身の観測は存在しない。

## 要約

| 区分 | 件数 | ID |
| --- | --- | --- |
| P1 (データ破壊 / クラッシュ / 誤削除 / 永久固着) | **0** | — |
| P2 (誤動作だが復旧可能) | **5** | C-1, C-2, C-3, M-1, M-2 |
| P3 | **12** | C-4〜C-11, M-3, M-4, M-5, M-7 |
| 懸念 (シナリオを書けない / 到達性未確定 / 設計判断) | **11** | K-1〜K-8, M-6, M-8, M-9 |

**総評**: stamp (context / surface generation / collection ID / accepted+wanted revision) の
所有分離、削除の fail-closed、参照解除と実ファイル削除の分離、migration の全 collection 単一
transaction といった**壊れると重い箇所は、コードを読んだ限り構造的に正しく作られている**。
P1 に相当する経路は見つからなかった。

**優先して見てほしいのは M-1**。`rename_item.rs` で file/folder を正しく型付けした
`CollectionSourceMigrationScope` が、**使われる前に `clear_rename_dialog_state()` で
`false` に潰され、production のファイル名変更が常に `Tree` で走る**。1 行で直る一方、
Exact 枝は production から到達不能なので、テストは通っているのに実機だけ違う経路になる。

P2 の C-1〜C-3 は同じ型に帰着する ——
**「一過性のストア状態 (`Busy` / `Starting` / 自分の保存が in-flight) を、恒久的な利用不可と
同じ値に潰している」**。潰した先が (a) 終端 `Failed`、(b) 無言の drop、(c) 「次の項目が無い」と
区別できない `Option::None` の 3 通りに分かれている。修正方向は 1 つで、
`collection_store_client_for_migration` (`src/ui_dialogs/collections.rs:1642`) が既に
`Result<Option<_>, CollectionStoreError>` で正しく型付けしているので、**read 経路の入口を
その型へ揃える**のが構造的修正になる。

---

## P2

### M-1 (P2) — ファイル名変更の migration scope が常に `Tree` になる (`Exact` 枝が production から到達不能)

**根拠**
- `src/ui_dialogs/rename_item.rs:46` — `self.rename_target_is_file = target.is_file();` で
  リネーム**前**に正しく捕捉する (`is_dir` からの事後再導出はしていない)。
- `src/ui_dialogs/rename_item.rs:69` — dialog 用にローカル `target_is_file` へコピー。
- `src/ui_dialogs/rename_item.rs:117` — 実行直前に `self.clear_rename_dialog_state();`。
  その中身 (`src/ui_dialogs/rename_item.rs:199-203`) が **`self.rename_target_is_file = false;`**。
- `src/ui_dialogs/rename_item.rs:118-120` — 直後に `self.rename_pending = Some(...)`。
- `src/ui_dialogs/rename_item.rs:132-136` — `poll_rename_pending` が
  `if self.rename_target_is_file { Exact } else { Tree }` を読む。**この時点では常に `false`**。
- 再設定経路は無い: `request_rename_dialog` は `rename_pending.is_some()` で再入を弾く
  (`src/ui_dialogs/rename_item.rs:36-39`)。`rename_target_is_file` の代入箇所は
  `src/ui_dialogs/rename_item.rs:46` と `:202`、初期化の `src/app.rs:16177` のみ (grep 済み)。
- 伝播先: `src/ui_dialogs/rename_item.rs:157` → `spawn_rename_key_migration`
  (`src/app.rs:31610-31626`) の `scope == Tree` で `PathMigrationJob::shell_rename(.., tree=true)`。
  production の呼び出し元はこの 1 箇所だけ (grep 済み)。

**失敗シナリオ**
1. テキスト import で `C:\photos\album.zip\inner.jpg` のような**ファイルの下にぶら下がる
   パス**を登録する。実在しないので `Unresolved` として保持される
   (`src/collection_store/prepare.rs:301-307`、仕様どおり missing は捨てない)。
2. 同じコレクションに実ファイル `C:\photos\album.zip` も登録されている。
3. mIV 上で `album.zip` を `album2.zip` にリネームする。
4. scope が `Tree` なので `CollectionSourceMigration::replacement_for`
   (`src/collection_store/path.rs:114-131`) の prefix 枝が成立し、
   `c:/photos/album.zip/inner.jpg` → `C:\photos\album2.zip\inner.jpg` へ**利用者が指示して
   いない参照の書き換え**が起きる。仕様の「UIから自動 / 推測再リンクは行わない」に反する。

**既存テスト**: **無し**。`src/app/tests.rs:52376-52384` は `spawn_rename_key_migration` を
scope 引数付きで**直接**呼ぶので、`poll_rename_pending` の scope 決定を通らない。
`CollectionSourceMigrationScope::Exact` を参照する他のテストは `src/collection_store/tests.rs`
の DB 層のみ。**テストが壊れた配線を迂回している**形。

**修正方向**: `target_is_file` を `rename_pending` 側 (または
`RenameMigrationPending { rx, scope }` のような typed pending) に載せる。
`clear_rename_dialog_state()` は dialog の state を消す関数なので、
migration の入力をそこに置かない。あわせて `poll_rename_pending` を通す handler-level
回帰 (file → Exact / folder → Tree) を追加する。

---

### C-1 (P2) — 読み取り専用の collection 経路が「並べ替え window の保存中」に巻き込まれ、スライドショーが末尾扱いで停止する

**根拠**
- `src/ui_dialogs/collections.rs:527-533` `can_edit()` = `phase == Ready` **かつ**
  `reorder.phase.is_busy()` (= `Saving` / `Refreshing`) でないこと。
- `src/ui_dialogs/collections.rs:1635-1640` `collection_store_client()` =
  `can_edit().then(|| client.clone()).flatten()`。
- この client を使う production 経路は**すべて読み取り専用** (`subscribe` / `load_collection`):
  `src/app/collection_grid.rs:878`(open)、`:922`(snapshot 要求)、`:1055`(watch 再購読)、
  `src/app/collection_navigation.rs:853`(ナビゲーション要求)。
- `src/app/collection_navigation.rs:853-866` — client が `None` / `subscribe` 失敗 /
  `load_collection` 失敗のいずれでも `finish_collection_navigation_without_target()` を呼ぶ。
- `src/app/collection_navigation.rs:2777-2781` — その `Slideshow` 分岐は
  `stop_slideshow_playback()` + `release_fs_nav_lock()`。つまり**「次が無い」と同じ終端**。

**失敗シナリオ**
1. コレクション C を開き、上部メニューから「現在のコレクションを並べ替え…」で専用 window を開く。
2. 1 件ドラッグして dirty にし、`閉じる` または X を押す → `start_collection_reorder_save()`
   (`src/ui_dialogs/collections.rs:1072`) で `CollectionReorderPhase::Saving` に入る。
   `poll_collection_reorder` は `request_repaint_after(50ms)` なので、この phase は
   最低 1 フレーム続く。
3. その間にコレクションのスライドショーが次コマへ進もうとする、または利用者が → / ↓ を押す。
4. `collection_store_client()` が `None` → **スライドショーが「コレクション末尾」として停止する**
   / 手動送りは `fs_boundary_hint` を出して無反応になる。エラーは一切出ない。

同じ経路で Grid 側は C-2 と合流して終端 `Failed` になる。

**既存テスト**: 無し。`dirty_reorder_close_autosaves_before_the_window_closes`
(`src/ui_dialogs/collections.rs:5213`) は `Saving` phase の保持だけを固定し、
その間の他経路への影響は見ていない。

**修正方向**: read 用アクセサを編集可否から分離する。`can_edit()` は mutation 入口
(`apply_collection_ui_action` / `add_grid_selection_to_collection` /
`start_collection_grid_content_action`) 専用にし、`subscribe` / `load_collection` は
`collection_store_client_for_migration` と同じ typed な read アクセサへ通す。
少なくとも `reorder.phase.is_busy()` を read 経路の条件から外す。

---

### C-2 (P2) — 一過性の `Busy` / 利用不可を Grid の終端 `Failed` へ格上げし、自力復帰しない

**根拠**
- `src/app/collection_grid.rs:922-929` — `collection_store_client()` が `None` なら
  `CollectionGridLoadState::Failed { message: "コレクションを利用できません" }`。
- `src/app/collection_grid.rs:940-948` — `client.load_collection()` の `Err` を種別に関係なく
  `Failed { collection_grid_error(&error) }` にする。`collection_grid_error`
  (`src/app/collection_grid.rs:1602-1609`) は `Busy` → 「コレクション処理が混み合っています」。
- `Failed` からの脱出は 2 つだけ: ① `cancel_pending()` (`src/app/top_level_grid_view.rs:563`) を
  呼ぶ **自 collection の revision 前進 notice** (`src/app/collection_grid.rs:1078-1083`)、
  ② `open_collection_grid` の再実行。`poll_collection_grid` の match
  (`src/app/collection_grid.rs:1274-1280`) は `Failed` に対して何もしない。
- `schedule_collection_grid_snapshot` (`src/app/collection_grid.rs:907-918`) は
  `RequestNeeded` のときしか再要求しないので、**自動 retry の owner が居ない**。

**失敗シナリオ**
- コマンドキューは `COMMAND_CAPACITY = 128` (`src/collection_store/runtime.rs:16`)。
  Remote 側は 1 request あたり最大 16 回 `subscribe` + `list_catalog`/`load_collection` を
  投げる (`src/remote_ipc/persistent_collections.rs:671-695, 698-730` の `MAX_EXACT_RESTARTS`)。
  端末が複数、あるいは連続スクロールで queue が瞬間的に飽和した状態で PC 側でコレクションを
  開く → `Err(Busy)` → Grid が **「コレクション処理が混み合っています」で固着**。
  その collection に mutation が起きない限り自力では戻らず、利用者が開き直すまで一覧が出ない。
- C-1 と合成すると: 並べ替え保存中に revision notice が来て `cancel_pending()` →
  `RequestNeeded` → `schedule_collection_grid_snapshot` → client `None` →
  **「コレクションを利用できません」の終端 Failed**。実際にはストアは生きている。

**既存テスト**: 無し (テスト棚卸しでも「actor queue 満杯 (Busy) が UI にどう見えるか」は
未カバー)。`failed_runtime_keeps_stale_snapshot_read_only_and_toolbar_reports_unavailable`
(`src/ui_dialogs/collections.rs:5794`) は runtime `Failed` (= 恒久) のみを固定している。

**修正方向**: `CollectionStoreError` を「一過性 (`Busy` / `Starting`)」と「終端 (`Unavailable` /
`IncompatibleSchema` / `Persistence`)」へ typed に二分し、一過性は `Failed` ではなく
`RequestNeeded` + 次フレーム再要求 (+ `request_repaint_after`) にする。
管理 UI 側は既にそう書かれており (`src/ui_dialogs/collections.rs:592, 633`)、**同じ判断が
2 箇所で別実装になっている**ことが問題の本体。

---

### C-3 (P2) — 管理 UI の catalog / snapshot 要求が `Busy` / `Starting` を無言で捨て、再駆動 owner も repaint 予約も無い

**根拠**
- `src/ui_dialogs/collections.rs:592` / `:633` — `Err(Starting | Busy) => {}`。
  `wanted_catalog_revision` / `wanted_collection_revision` だけ進み、request は作られない。
- 再駆動は `poll_collection_ui` の `if let Some(request) = ...catalog_request.take()`
  ブロックの**内側** (`src/ui_dialogs/collections.rs:1745-1755`, `:1777-1789`) にあるため、
  request が作られなかった回は走らない。
- 末尾の repaint 予約 (`src/ui_dialogs/collections.rs:1794-1801`) の条件は
  `Starting || catalog_request.is_some() || snapshot_request.is_some() || !operation.is_idle()`。
  **phase = Ready・operation idle で `Busy` を食った瞬間は repaint も予約されない**。
  mIV は静止時に就寝する設計なので、次の入力まで frame が来ない可能性がある。

**失敗シナリオ**
1. Remote からの request で actor queue が一時的に満杯になる。
2. 同じタイミングで revision notice が届く (`src/ui_dialogs/collections.rs:1708-1723`)。
   `take_latest()` で notice は**消費済み**になる。
3. `request_catalog` が `Busy` で無言終了。repaint も予約されない。
4. → **管理 window の一覧 (名前・revision) が古いまま止まる**。利用者が何か操作するまで戻らない。
   古い revision で 名前変更 / 削除を押すと `Conflict` になる (データ破壊はしない)。

**既存テスト**: 無し。

**修正方向**: `Busy`/`Starting` を食った場合も再駆動対象として記録し、
`ctx.request_repaint_after` を予約する。C-2 と同じ「一過性 error の typed な扱い」に統合する。

---

### M-2 (P2) — ジャーナル読み込みの I/O 失敗が「空」と同一視され、その後ジャーナルごと削除される

**根拠**
- `src/rename_key_migration.rs:235-238`
  ```rust
  let Ok(bytes) = std::fs::read(&path) else { return Vec::new(); };
  ```
  **ログも無く**空として扱う (JSON 破損時は `:249-253` でログが出るのに、read 失敗だけ無言)。
- `src/rename_key_migration.rs:260-268` — `journal_save` は `entries.is_empty()` なら
  `remove_file` する。
- `src/app.rs:31516` — `resolve_collection_migration_for_exit` の `other` 枝が
  in-flight 無しでも `persist_rename_migration_journal_attempt(0)` を無条件に呼ぶ。

**失敗シナリオ**
1. 前セッションで名前変更 → migration が未完のままクラッシュ / 強制終了。ジャーナルに残る。
2. 次の起動時、AV・インデクサ・バックアップが一瞬 `rename_migration_journal.json` を
   ロックし、`ensure_rename_migration_journal_loaded` (`src/app.rs:31543-31560`) の
   `journal_load` が `Err` → **空として扱われ、ログも残らない**。
3. そのセッションの終了時 (あるいは次の enqueue 時) に空スナップショットが persist され、
   **ジャーナルファイルが削除される**。
4. → 前セッションの未完了 migration (★/タグ/履歴の path-key と **collection 参照**) が
   恒久的に失われる。ファイルは新 path にあるので、collection 側は該当 entry が
   `見つかりません` になる。手動の登録解除→再追加でしか戻せない。

**既存テスト**: 無し。`journal_roundtrip_and_cleanup` (`src/rename_key_migration.rs:2309`) は
正常系のみ。

**修正方向**: `journal_load` の戻り値を `Result` にし、read 失敗を「空」と区別する。
読めなかったセッションでは `journal_save` の空削除を抑止する (最低でも `logger::log` を足す)。
ジャーナル自体はコレクション以前からある機構だが、collection 参照が乗ったことで損失の重みが
上がっている。

---

## P3

### C-4 (P3) — notice に自 collection が無いことを、catalog revision を比べずに「削除」と断定する

**根拠**
- `src/app/collection_grid.rs:1062-1090` — `session.observed_catalog_revision` は
  `CollectionGridSession::new` で **0 初期化** (`src/app/top_level_grid_view.rs:533`)。
  `notice.catalog_revision > 0` を満たす最初の notice で、`collection_revisions` に自 ID が
  無ければ無条件に `load = Deleted` + 「コレクションは削除されました」を表示する。
- 同型が `src/app/collection_navigation.rs:489-500` `observe_revision()` にもある
  (不在 → `RevisionObservation::Deleted` → ナビゲーション中断)。
- `CollectionStoreClient::subscribe()` (`src/collection_store/runtime.rs:446-472`) は
  `hub.latest` を baseline として即座に配る。`hub.latest` の更新は `process_command` で
  **reply を send した後**に行われる (`src/collection_store/runtime.rs` 末尾 `if mutated { publish_revision }`)。
- App は authoritative な catalog revision を持っている (`collection_ui.catalog.catalog_revision`)
  のに、session の `observed_catalog_revision` に seed していない。

**失敗シナリオ (到達性 未確認)**
Create の reply を UI が受け取ってから actor が `db.catalog()` + `publish_revision` を終える
までの窓で `subscribe()` すると、**新しい collection を含まない baseline notice** が返り、
session が `Deleted` を表示する。ただし `poll_collection_actor_task::Create`
(`src/ui_dialogs/collections.rs:2255-2261`) は Grid を開かないため、同一フレームで
連続する production 経路は見つけられていない。したがってこれは
**「不変条件が revision 比較ではなく『reply が publish より先』という暗黙の順序に
依存している」という構造上の指摘**であり、再現シナリオは提示できない。

**既存テスト**: 無し。実装記録 §21 に「subscribe 時の baseline notice を rename notice と
誤認する race を 1 件検出した」とあり、同じ面に既に 1 度踏んでいる。

**修正方向**: session 作成時に `observed_catalog_revision` を App の authoritative catalog
revision で seed する。または「不在 = 削除」を
`notice.catalog_revision >= (自 collection の存在を確認済みの catalog revision)` のときだけ成立させる。

### C-5 (P3) — revision watch の subscriber が mutation 時にしか刈られず、閲覧だけで単調増加する

**根拠**: `src/collection_store/runtime.rs:470` の `subscribe()` は
`hub.subscribers.push(Arc::downgrade(&slot))` するだけで、死んだ `Weak` の除去は
`publish_revision` の `retain` (`src/collection_store/runtime.rs:1031-1039`) **だけ**。
`publish_revision` は mutation 時にしか呼ばれない。呼び出し頻度は
`src/app/collection_navigation.rs:859` (ナビゲーション 1 回ごと = ページ送り / スライドショー
1 コマ / 動画・音声 EOF) と `src/remote_ipc/persistent_collections.rs:680, 709`
(Remote request ごと、restart loop で最大 16 回)。

**失敗シナリオ**: 編集を一切せずにコレクションのスライドショーを長時間流す / Remote で
長時間閲覧すると `hub.subscribers` が単調に伸びる。1 要素あたり `Vec` 領域 16B +
解放できない `RcBox<RevisionWatchSlot>` 数十 B 程度。10,000 コマで 1MB 弱、Remote の
長時間セッションではその 16 倍のオーダー。次の mutation 時に `publish_revision` が
全長を retain するのでロック保持時間も伸びる。クラッシュには至らない見込み。

**既存テスト**: 無し。**修正方向**: `subscribe()` 側でも `retain` する。または
navigation / Remote が request ごとに subscribe せず owner ごとに 1 本の watch を持ち回す。

### C-6 (P3) — text import が `%` を 2 個以上含むパスを「環境変数式」として一律拒否する

**根拠**: `src/collection_store/path.rs:300-309`
(`bytes.iter().filter(|byte| **byte == b'%').count() >= 2`)。

**失敗シナリオ**: Web から保存した percent-encoded ファイル名
(`C:\dl\%E3%81%82%E3%81%84.jpg`) や `C:\sale\50%_off\100%.jpg` を並べたテキストを import すると、
**全行が Invalid** になり Confirm が無効化される。利用者は理由を理解できない。

**既存テスト**: `external_text_policy_rejects_device_verbatim_and_root_escape`
(`src/collection_store/tests.rs:41`) が `%USERPROFILE%\a.jpg` の拒否を**仕様として固定済み**。
粗い判定は意図的だが、誤検知側は固定されていない。
**修正方向**: `%[A-Za-z_][A-Za-z0-9_()#]*%` にマッチする場合だけ拒否する。

### C-7 (P3) — 末尾ドット付きパスが同一ファイルを二重登録できる

**根拠**: `src/collection_store/path.rs:215-251` は component の trailing `.` / 空白を
除去しない。一方 `is_dos_device_component` (`:313-314`) は `trim_end_matches([' ', '.'])` しており、
Windows のこの正規化自体は認識されている。

**失敗シナリオ**: `C:\pic\a.jpg` 登録済みのコレクションへ `C:\pic\a.jpg.` と書いた
テキストを import する。Win32 のパス正規化により `std::fs::metadata`
(`src/collection_store/prepare.rs:559`) は同じファイルを見つけ `Available{Image}` になる。
一方 source key は `c:/pic/a.jpg.` ≠ `c:/pic/a.jpg` なので UNIQUE に掛からず、
**同じファイルが 1 つのコレクションに 2 行出る** (仕様「同じ参照は一つのコレクション内で
重複登録しない」に反する)。直接追加と migration は列挙済み実パス由来なので影響しない。
末尾**空白**は parser の `original.trim()` (`src/collection_store/text.rs:47`) が先に落とす。

**既存テスト**: 無し。**修正方向**: `normalize_collection_path` の component 生成時に
最終 component も含めて `trim_end_matches([' ', '.'])` を適用する (空なら `InvalidPath`)。

### C-8 (P3) — import が UTF-8 以外を一切受け付けず、失敗文言に std の英語メッセージが出る

**根拠**: `src/ui_dialogs/collections.rs:2884` `std::fs::read_to_string(&worker_path)` →
`.map_err(|error| format!("インポートファイルを読めませんでした: {error}"))`。

**失敗シナリオ**: PowerShell 5.1 の `Get-ChildItem | % FullName > list.txt` は既定で
**UTF-16 LE (BOM 付き)** を書く (CLAUDE.md「エンコーディング」節と同じ事情)。この list.txt を
import すると 1 行も解析されず「インポートファイルを読めませんでした: stream did not contain
valid UTF-8」と表示される。CP932 保存も同じ。データは壊れない (fail closed) が原因が伝わらない。
併せてサイズ上限が無いので、誤って巨大ファイルを選ぶと worker が全量を確保する。

**既存テスト**: 無し。**修正方向**: UTF-16 BOM を検出したら typed な案内を出す。
少なくとも std の英語文字列をそのまま連結しない。

### C-9 (P3) — 終了時の collection migration 待ちに deadline が無い

**根拠**: `src/app.rs:31527` `match rx.recv() {` — `resolve_collection_migration_for_exit()`
(`src/app.rs:31505`) が in-flight の migration 応答を**タイムアウト無し**で待つ。
`App::on_exit` (`src/app.rs:74559`) から呼ばれる。actor が死ねば sender drop で `Err(_)` に
落ちて boot retry へ回るので恒久ハングにはならないが、同じ終了経路の TRT / Remote は
deadline を持っている。

**失敗シナリオ**: 巨大な catalog に対する `migrate_source_batch`
(`src/collection_store/db.rs:363` の `load_all_entries` は**全 collection の全 entry**を読む) や、
その手前に Remote の request が queue に並んでいる状態でアプリを閉じると終了が数秒伸びる。
利用者には「閉じても消えない」ように見える。

**既存テスト**: 無し。**修正方向**: bounded な `recv_timeout` にし、期限切れは
既存の `Err(_)` 分岐と同じく `rename_migration_boot_retry` へ回す。

### C-10 (P3) — 大規模コレクションで、1 件の操作が全件走査になる箇所が 4 つある

**根拠**
1. `src/collection_store/db.rs:727-744` `compact_manual_positions` — `remove_entries` のたびに
   **collection の全行を 2 回 UPDATE**。`manual_position` は `ORDER BY ... ASC` でしか
   使われないので歯抜けを許しても正しさは保たれる。
2. `src/collection_store/db.rs:363-372` `migrate_source_batch` — `load_all_entries` が
   **全 collection の全 entry** をメモリへ読む。mIV 内のファイル名変更 1 回ごとに発生。
3. `src/collection_store/prepare.rs:374-400` — prepare が**全 entry に `std::fs::metadata`**。
   revision が 1 進むたびに mount 済みの各 viewer context と Remote がそれぞれ全件やり直す。
4. `src/collection_store/model.rs:365-390` `effective_collection_order` — facts 1 件ごとに
   `snapshot.entries.iter().any(...)` で O(n²)。

**失敗シナリオ**: 5,000 件のコレクションから 1 件を「コレクションから外す」と、actor が
10,000 回の UPDATE を 1 transaction で実行する間、同じ actor に並んだ他の request
(別 context の `load_collection`、Remote の閲覧) が待たされる。ネットワーク共有を含む
コレクションでは 1 件追加ごとの再 prepare が全件 stat になる。C-2 の `Busy` 固着とも合流する。

**既存テスト**: 無し。**修正方向**: (1) は削除で歯抜けを許す (`add_batch` は既に `MAX+1`:
`src/collection_store/db.rs:415-420`)。(4) は `HashSet` で ID 集合を先に作る。
(2)(3) は規模の想定を仕様側で決める。

### C-11 (P3) — binding が stale な間、context menu から「コレクションから外す」だけが消え、「元ファイルをゴミ箱へ移動」が残る

**根拠**: `src/ui_dialogs/context_menu.rs:1351` は
`collection_reference: ...ready_target().is_some()` (Ready のときだけ)、`:1352` は
`collection_source_context: ...is_collection_root()` (Unavailable でも true)。
`src/context_menu_model.rs:1427-1447` は `collection_reference` が false だと
`RemoveFromCollection` を出さないが、`MoveToRecycleBin` は `kind.supports_delete()` だけで出る。
stale になる代表経路は `invalidate_current_collection_grid_sources`
(`src/app/collection_grid.rs:312-332`) が `installed_items_generation = None` にする一方、
`self.items` は古い行のまま残ること。再 prepare は全件 stat (C-10) なので窓は短くない。

**失敗シナリオ**: コレクション直下でファイルをごみ箱へ送った直後 (= 再分類中) に別のセルを
右クリックすると、**安全な「コレクションから外す」が消え、破壊的な「元ファイルをゴミ箱へ移動
(タグ・評価も整理)」だけが残る**。Delete キーは fail closed でトーストを出す
(`src/ui_dialogs/context_menu.rs:2095-2102`) ので誤削除はしないが、メニューは誘導的になる。

**既存テスト**: Delete キー側の fail closed は `src/app/collection_grid.rs:1914` と
`src/ui_dialogs/collections.rs:4155` が固定。context menu 側で「安全な項目だけ消える」状態を
固定したテストは無い。**修正方向**: stale のときは `RemoveFromCollection` を
`enabled: false` + `disabled_reason` で残し、`MoveToRecycleBin` も同じ理由で無効化する
(両方を 1 つの typed resolution から導く)。

### M-3 (P3) — ジャーナル保存が恒久失敗すると、セッション中の全 migration が停止し 10Hz の再描画ループが残る

**根拠**: `src/app.rs:31406-31421` の `RETRY_DELAYS` は 3 要素 (100ms / 1s / 4s)。
枯渇すると `retry_attempt = None` になり、`src/app.rs:31491-31498` が以後 retry しない。
通知は `reported` により 1 回だけ (`src/app.rs:31474-31490`)。一方
`src/app.rs:31984-31986` は `queue` が非空なら**無条件に** `request_repaint_after(100ms)` する。

**失敗シナリオ**: data_dir が書込不能 (ネットワーク断・権限変更) になると、そのセッション中は
リネーム移行が一切開始されず、通知も 1 回きり。同時に queue が非空のまま残るので
**静止時も 10Hz で起き続ける** (CLAUDE.md の idle-health 方針に反する)。

**既存テスト**: 部分的。`unsaved_rename_journal_never_admits_collection_migration_stage`
(`src/app/tests.rs:23324`) は**一過性失敗からの回復**のみ。予算枯渇後の恒久停止と
再描画ループは未検証。

### M-4 (P3) — 決定的エラー (`DuplicateSource` / `InvalidPath`) が毎起動リトライされ続ける

**根拠**: `src/collection_store/db.rs:381-390` が `resulting_keys` の一意性違反で
`DuplicateSource` を返しバッチ全体をロールバック。`src/app.rs:31958-31968` が
`rename_migration_boot_retry` へ回し、ジャーナルに残す。boot_retry はセッション内では
再投入されない (`try_start_next_rename_migration` は `rename_migration_queue` だけを見る、
`src/app.rs:31691-31697`)。`InvalidPath` も同様 (`src/app.rs:31770-31783`)。

**失敗シナリオ**: コレクション X に `C:\a\one.jpg` と `C:\a\two.jpg` が登録済みの状態で、
Shell の上書き確認を通して `one.jpg` → `two.jpg` にリネームする。migration は
`DuplicateSource` で必ず失敗 → 次回起動でも同じ結果 → **起動のたびにトーストが出続け、
ジャーナルが消えない**。製本の並べ替え / 転送のように 1 job に多数 mapping が入る場合、
**1 件の重複で全 mapping が移行されない**。

**既存テスト**: DB 層のロールバックは
`migration_batch_swaps_sources_in_one_transaction_and_rolls_back_duplicates`
(`src/collection_store/tests.rs:494`) で固定済み。App 側の恒久リトライ挙動は未検証。
**修正方向**: 決定的エラーは boot_retry ではなく terminal として記録し、1 回だけ報告して
ジャーナルから外す (仕様「unique conflict は勝手に merge / delete せず報告する」は満たしつつ、
無限リトライを止める)。

### M-5 (P3) — 製本系で FS が完全にコミットされた後にエラーを返し、collection migration が起きない

**根拠**: `src/books.rs:174-178` — `execute_forward` (FS 適用) が全段成功した**後**に
`journal.mark_filesystem_committed(plan.len()).map_err(...)?` が `Err` を返し得る。
その場合 `src/app.rs:35004-35007` の `Err` 枝に落ち、`apply_book_page_edit_moves_with_journal`
(= collection migration の enqueue) は呼ばれない。worker 消失 (`src/app.rs:34843-34845`) と
rollback 失敗 (`src/books.rs:165-170`) も同型。

**失敗シナリオ**: 本の並べ替え / 転送でページファイルは新 path へ移動済みなのに、
コレクション参照は旧 path のまま → 該当 entry が `見つかりません` になる。
**方向としては安全側** (仕様「失敗時に参照だけ進むことがないように」を満たす) だが、
FS は進んだのに参照が取り残される。

**既存テスト**: 無し (`flush_reorder_rolls_back_when_final_name_conflicts`
(`src/books.rs:3511`) は rollback 成功系のみ)。

### M-7 (P3) — ジャーナルの temp ファイルが fsync されない (同リポジトリ内で方針不一致)

**根拠**: `src/rename_key_migration.rs:270-277` は `std::fs::write(&tmp, &json)` の後
`sync_all` 無しで `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` を呼ぶ。
`MOVEFILE_WRITE_THROUGH` は rename のメタデータのみを保証する。
同じリポジトリの collection export (`src/collection_store/prepare.rs:544-552`) は
`flush` + `sync_all` してから replace しており、**2 箇所で方針が違う**。

**失敗シナリオ**: 電源断で「名前は正しいがデータが空 / 部分的」なジャーナルが残り得る。
M-2 (read 失敗 = 空扱い) / JSON parse 失敗 (`src/rename_key_migration.rs:249-253`) と
合流すると、未完了 migration が失われる。

**既存テスト**: `journal_atomic_replacement_overwrites_an_existing_snapshot`
(`src/rename_key_migration.rs:2416`) は置換の原子性のみ。
**修正方向**: collection export 側に合わせて `sync_all` を入れる。

---

## 懸念 (失敗シナリオを書けない / 到達性未確定 / 設計判断)

### K-1 — `collection.db` に世代バックアップも破損時の復旧導線も無い
`src/collection_store/db.rs:25-56` の `open_at` は、破損ファイル (SQLite として読めないバイト列)
に対し `pragma_query_value` が失敗して `Persistence(..)` → `Failed` になる。
**空カタログで黙って置き換えないのは正しい** (plan §3.2 の要件を満たす)。
ただし `settings.db` が bak1..bak10 + quarantine を持つのに対し、コレクションは
plan §3.3 のとおり明示的に backup 対象外で、text export も「手動 portability であって
backup ではない」と定義されている。利用者が手で組み上げた名前付きコレクションが
1 ファイルの破損で全損し、アプリ内の復旧導線が無い。仕様どおりだが、v4.0.0 の目玉機能として
この扱いでよいかは利用者判断を仰ぐ価値がある。テストは
`newer_schema_is_not_replaced_with_an_empty_catalog` (`src/collection_store/tests.rs:942`) が
**schema version 不一致のみ**を固定しており、破損バイト列は未カバー。

### K-2 — 並べ替え window の X / 「閉じる」が dirty 時に自動保存する
`src/ui_dialogs/collections.rs:3663-3678` — `!window_open` かつ `dirty` なら `save = true`。
`dirty_reorder_close_autosaves_before_the_window_closes` (`:5213`) で意図的に固定されている。
一方 `Error` phase には明示の「変更を破棄して閉じる」がある (`:3302`)。
X を「取り消し」と解釈する利用者にとっては確認なしに順序が確定する。仕様判断。

### K-3 — 同じ意味の文言・述語が 2 箇所に別綴りで存在する
- `collection_error_message` (`src/ui_dialogs/collections.rs:697-731`) と
  `collection_grid_error` (`src/app/collection_grid.rs:1602-1609`) が同じ 3 語を句点の有無で
  別々に持ち、後者の fallback は `CollectionStoreError` の `Display` (内部語) をそのまま UI へ出す。
- 「同じパスか」の述語が 2 系統ある: collection の `CollectionSourcePathKey`
  (`src/collection_store/path.rs`) と、delete 無効化が使う `crate::path_key::normalize_keep_drive`
  (`src/app/collection_grid.rs:1582-1600`)。現状は両辺とも同じ関数を通しているので内部整合は
  取れているが、CLAUDE.md「One Owner Per Spelling」の観点では 2 つ目の綴りが増えている。

### K-4 — 未知の `resolved_kind` / `sort_order` 文字列が catalog 全体の読み込みを失敗させる
`src/collection_store/db.rs:612-640` / `:823-845` — `from_str` が `None` を返すと
`FromSqlConversionFailure` になり、`load_snapshot` / `load_catalog` **全体**が `Persistence` error に
なる。schema version gate があるので正常な将来版では起きないが、「新しい kind を足す → 旧版では
1 行だけ Unresolved に degrade」ではなく「旧版ではコレクション機能ごと利用不可」という硬い形。
初回リリース後に `resolved_kind` を増やしにくい。将来 nested collection や ZIP 内ページ登録を
足す余地 (`source_namespace` は enum 1 値で拡張余地あり、`collection_entries` に entry 種別列は無い)
と併せて設計判断として記録する。

### K-5 — コレクション名に一意制約が無い
`src/collection_store/db.rs:190-196` の `collections` に `UNIQUE(name)` は無く、
`create_collection` (`:75-97`) も重複名を弾かない。ID が stable なので機能上の問題は無いが、
toolbar の追加先コンボ / 固定ショートカット / 管理 window が同名で並ぶ。意図的か未確認。

### K-6 — Backward + Loop が anchor 自身を候補に含めない (現状は到達不能)
`src/collection_store/prepare.rs:186-196` — Forward + Loop は `indices.extend(0..=position)` で
anchor を含むのに、Backward + Loop は `((position + 1)..len).rev()` で **anchor を除く**。
要素 1 件のコレクションで Backward + Loop は候補 0 件になる。ただし
`CollectionNavigationAction::tail()` (`src/app/collection_navigation.rs:112-134`) で `Loop` を
返すのは Slideshow(LoopFolder) / Video / Music の EOF だけで、いずれも `direction() == Forward`
(`:81-92`) のため **現在の production では Backward × Loop は組まれない**。
将来の潜在バグとして記録する。テストも Backward × Loop の組は 0 件。

### K-7 — App 終了時、actor に enqueue 済みの command が `Unavailable` で捨てられる
`src/collection_store/runtime.rs:855-866` の `select_biased!` は control (Shutdown) を優先し、
その後 queue 済み command を全て `Unavailable` にする。
`actor_prioritizes_shutdown_over_a_command_queued_behind_in_flight_work`
(`src/collection_store/tests.rs:824`) と `shutdown_rejects_queued_commands_without_mutating_them`
(`:791`) で**意図として固定済み**。ただし `docs/collection-implementation-plan.md` §16.5 は
「既にactorへenqueue済みの短いcommandだけをresultまたはactor terminal ACKまで処理する」と
書いており、**文書と実装が食い違っている**。実害の窓は `begin_shutdown` が `src/lib.rs:1514`
(on_exit より後) で呼ばれるため極めて狭く、migration は M-2 を除き journal で次 boot へ回る。
文書側を実装に合わせるのが妥当と思われる。

### K-8 — export ファイルに BOM が無い
`src/collection_store/text.rs:100-114` — `serialize_collection_paths` は UTF-8 (BOM なし) + CRLF。
import 側は BOM を許容する (`src/collection_store/text.rs:44`) ので往復は成立する。
CLAUDE.md「外部ツールに渡すテキストには BOM を付ける」という同リポジトリの方針
(PowerShell 5.1 / 旧メモ帳が ANSI と誤読する) に照らすと export も対象に当たる。
Win11 の既定メモ帳は UTF-8 を検出するので実害は限定的。仕様判断。

### M-6 — migration 完了処理から到達する明示 `panic!`
`src/app.rs:32070-32075` —
`.unwrap_or_else(|error| panic!("rename rehydrate context {window_id} failed to mount: {error:?}"))`。
汎用ステージ完了後の `rehydrate_contexts_after_rename_migration` が detached window の context を
mount できないとプロセスが落ちる。`window_ids` の収集 (`:32054-32068`) と実行の間に
他スレッドの変更は入らないので現状は到達しない可能性が高いが、
「移行完了の副作用でアプリを落とす」設計であること自体がリスク。到達性は確定できていない。

### M-8 — `CollectionRuntimePhase::Inert` で migration job が無言破棄される
`src/app.rs:31786-31793` — `collection_store_client_for_migration` が `Ok(None)` (= Inert) を
返すと job を pop して**ログもトーストも無し**に破棄する。`Inert` は
`CollectionUiState::default()` (`src/ui_dialogs/collections.rs:506`) の初期値で、
production では `src/lib.rs:1400-1404` が App 生成時に必ず install するため
`Starting`/`Failed` になり到達しないと判断する。ただし `Ok(None)` を「破棄してよい」と
解釈する構造は、将来 install 経路が増えたときに静かな欠落を作る。

### M-9 — クラッシュ窓での再実行が「旧 path の新規登録」を巻き込む
`src/app.rs:31950-31957` — 成功時のジャーナル消し込みは非同期 persist なので、
A→B の migration が actor でコミット済み・ACK 前にプロセスが死ぬと、次回起動で A→B が
再実行される。その再実行待ちの間に利用者が path A を (同名ファイル再作成などで)
コレクションへ登録すると、**その新しい entry が B へ移動する** (`src/collection_store/path.rs:108-113`
の exact 一致は現在の登録内容だけで判定するため)。B が既登録なら M-4 の恒久失敗になる。
再実行は通常起動直後の最初の poll で走るため窓は非常に狭く、実害は確認できていない。

---

## 監査したが問題なしと判断した項目

- **参照解除の fail closed**: `collection_root_delete_resolution`
  (`src/app/collection_grid.rs:369-440`) は surface / position=Root / stamp /
  `installed_items_generation == items_generation` / `prepared.collection_revision ==
  accepted_revision` / `entries.len() == items.len()` / 各 index の解決をすべて検査し、
  1 つでも欠けると `Unavailable` を返す。`handle_delete_key`
  (`src/ui_dialogs/context_menu.rs:2093-2103`) は `Unavailable` でトーストして `return` し、
  **物理削除へ fallback しない**。`NotCollectionRoot` だけが従来の file delete へ落ちる。
- **checked / selected の対象解決**: 対象 index はすべて同一 `prepared` から引くため、
  別 collection の項目や physical child が混ざる経路が無い。`db.remove_entries`
  (`src/collection_store/db.rs:238-267`) も `WHERE collection_id = ?1 AND id = ?2` なので、
  仮に他 collection の entry ID が渡っても削除できない (二重の fail-safe)。
- **定義削除が実ファイルを消さない**: `db.delete_collection`
  (`src/collection_store/db.rs:126-141`) は `DELETE FROM collections` + `ON DELETE CASCADE` のみ。
  `ActorTask::Delete` (`src/ui_dialogs/collections.rs:2352-2372`) も filesystem を呼ばない。
- **元ファイル削除で entry を消さない**: delete worker 完了時に呼ぶのは
  `invalidate_collection_grid_sources` (`src/app.rs:32384`) だけで、参照の remove は行わない。
  `remove_entries` の production 呼び出し元は UI の明示「登録解除」のみ
  (`src/ui_dialogs/collections.rs:3222`)。`metadata_cleanup` / `purge_removed_paths_at` の
  `STORES` に `collection.db` は含まれない (`src/rename_key_migration.rs:504-651`)。
  次の prepare で `CollectionPlaceholderReason::Missing` になる (= 仕様の missing 保持)。
- **delete scope の型付け**: `Exact` / `Tree` は削除**開始時**に `self.items` の
  `GridItem::Folder` 判定から決まる (`src/app.rs:32255-32267`)。削除後の `is_dir` からは
  推定していない。`source_scope_contains` (`src/app/collection_grid.rs:1582-1600`) は
  component 境界で判定し、`c:/a/b` の Tree が `c:/a/bc` に当たらない。
  (※ **rename 側の scope は M-1 で壊れている**。delete 側は正しい。)
- **rename migration の prefix 境界**: `CollectionSourceMigration::replacement_for`
  (`src/collection_store/path.rs:100-132`) は `old_prefix = "<key>/"` を使うので
  `C:\a\b` の Tree rename が `C:\a\bc` に誤一致しない。suffix は `component_suffix`
  (`:389-403`) が component 単位で取り、stored path と key が食い違えば `InvalidPath` で失敗する。
- **migration が FS 成功後にのみ enqueue される**: Shell rename は
  `src/ui_dialogs/rename_item.rs:131-161` の `Ok(Ok(outcome)) && !outcome.aborted` 枝のみ。
  `src/shell_file_ops.rs:112-135` が「新 path が存在し旧 path が消えている」を事後検証してから
  `Ok` を返す。製本系は `src/app.rs:34894, 34943, 34994`、`src/ui_main.rs:7201, 7242` の
  `Ok(BookOpResult::…)` 枝のみ。**「DB 参照が動いたのに FS が失敗」経路は見つからなかった**
  (逆方向は M-5)。
- **本の rename / ページ移動が 1 バッチで送られる**: `book_collection_source_mappings`
  (`src/app.rs:11113-11139`) → `PathMigrationJob::collection_only`
  (`src/rename_key_migration.rs:73-78`) → `CollectionSourceMigrationBatch`
  (`src/app.rs:31752-31769`) → 1 SQLite transaction。1 件ずつ送る production 経路は無い
  (`migrate_sources` 単数版の production 呼び出し元ゼロ)。
- **UNIQUE(collection_id, manual_position) の一時違反**: `reorder_manual`
  (`src/collection_store/db.rs:288-305`) と `compact_manual_positions` (`:727-744`) は
  先に全行を `-manual_position - 1` へ移す。`x → -x-1` は単射かつ全て負なので、
  0..n-1 への再採番と衝突しない。`compact_catalog_positions` (`:746-766`) も同型。
- **migration の一時 UNIQUE 違反**: `migrate_source_batch` (`src/collection_store/db.rs:404-428`) は
  対象行を先に `source_namespace='_migration', normalized_path=<entry_id>` へ退避してから
  最終値を入れる。entry ID は unique なので退避中も衝突しない。swap / cycle も 1 transaction。
  `resulting_keys` による事前検証 (`:378-392`) で UNIQUE 衝突を検出したら `DuplicateSource` で
  全体 rollback し、勝手に merge / delete しない (仕様どおり。無限リトライは M-4)。
- **schema version gate**: `open_at` (`src/collection_store/db.rs:25-56`) は read-only probe →
  version 判定 → **その後で** `foreign_keys` / `journal_mode=WAL` を設定する。
  未知 schema には WAL ファイルも作らない。`busy_timeout` は明示 3 秒。
- **catalog_revision と collection revision の commit 同時性**: 全 mutation が 1 transaction 内で
  `increment_collection_revision` → `bump_catalog_revision` → `load_snapshot_with_catalog` →
  `commit` の順に並ぶ。no-op / Conflict / rollback ではどちらも進まない
  (`no_op_and_conflict_do_not_advance_revision`)。
- **Standard 中の追加が manual tail に入る**: `add_batch` (`src/collection_store/db.rs:415-420`) は
  order mode を見ずに `MAX(manual_position)+1` から採番する。`set_order` (`:144-172`) は
  `manual_position` を触らない。
- **Manual のカテゴリ再配置なし**: `effective_collection_order`
  (`src/collection_store/model.rs:365-370`) は Manual なら DB 順をそのまま返す。
  Standard の `UnresolvedTail` は rank 4、`GridDisplayOrder::row_for`
  (`src/settings.rs:2286-2291`) は 4 行なので 0..=3 を返し、tail と衝突しない。
- **request stamp の stale 拒否**: `TopLevelGridView::begin` / `replace_surface`
  (`src/app/top_level_grid_view.rs:848-891`) は **必ず新しい `CollectionGridSession` を作る**ので、
  collection 切替時に古い `Snapshot` / `Preparing` が生き残らない。
  `CollectionGridSession::Drop` (`:611-620`) が prepare cancel と video worker cancel を必ず立てる。
  `Clone` (`:622-637`) は receiver / watch をコピーせず mount 後に再購読する。
- **prepare 受理条件**: `src/app/collection_grid.rs:1218-1224` が stamp / collection ID /
  `exact_revision` 完全一致 / `wanted_revision <= exact_revision` をすべて要求する。
  snapshot 側は `>= minimum_revision` かつ `>= wanted` (`:1155-1160`) で、2 段が別基準
  (plan §16.1 のとおり)。
- **UI スレッドの I/O**: `prepare_collection_snapshot` / `_while` /
  `prepare_collection_registrations` / `prepare_collection_export` / `inspect_collection_source` の
  production 呼び出しは全て worker thread か Remote worker。`poll_collection_grid`
  (`src/app/collection_grid.rs:1045`) は全分岐 `try_recv`。`try_lock + sleep` のアンチパターンは
  collection / journal 配下に見当たらない (journal writer は Mutex + Condvar、
  `src/rename_key_migration.rs:391-421`)。
- **物理 child 採用の順序**: `adopt_collection_surface_for_physical_load`
  (`src/app.rs:21410-21453`) が `position = PhysicalSource` にするのは scan 成功後で、
  直後の `load_zip_as_folder` / `load_pdf_as_folder` / `start_loading_items` が**同一呼び出し内で**
  items を差し替える。フレーム境界を挟まないので「position は child、items は root 行」という
  状態で Delete キーが動く窓は無い。scan 失敗 (`src/app.rs:21735-21740`) は adopt の前に return する。
- **本の終端**: `finish_collection_navigation_without_target`
  (`src/app/collection_navigation.rs:2759-2814`) は境界ヒント / トースト / slideshow 停止だけを行い、
  **物理 folder tree の DFS へ fallback しない**。Ctrl+↑↓ は ZIP 内の子 DFS を先に消費してから
  outer 順へ移る (`collection_physical_zip_uses_child_dfs_before_resuming_outer_order`)。
- **detached operation の横取り防止**: 管理 window の 作成 / 名前変更 / 削除 / 追加先変更は
  すべて `!busy` (= `operation.is_idle()`) でガードされ (`src/ui_dialogs/collections.rs:2624-2717`)、
  `open_collection_manager` (`:1400`) と `select_collection_management_target` (`:1662`) も
  `operation.is_idle()` を要求する。`select_collection` が in-flight の detached operation を
  破棄する経路は `install_catalog` の自動 fallback (`:657`) のみで、そちらも
  「選択中の collection が消える」= delete が必要、delete 自体が operation スロットを要求するため
  単一プロセスでは到達しない。
- **Remote への raw client 漏れ**: `CollectionStoreClient` を Remote へ渡す経路は
  `CollectionRemoteProducerControl` (`src/lib.rs:1296-1307`) の 1 本のみで、
  `PersistentCollectionEngine` は `producer` フィールドしか持たない
  (`src/remote_ipc/persistent_collections.rs:46`)。client の取得はすべて `lease.client()`。
  終了順は (b) collection migration drain (`src/app.rs:74558`) → (a) producer `close_and_drain`
  (`src/lib.rs:1510`) → (c) actor `shutdown_and_join` (`src/lib.rs:1513`) で、plan §4.2 の順序どおり。
  (b) が (c) より前なので、投入済み migration が `Unavailable` で拒否されかつジャーナルにも
  残らない窓は見つからなかった (M-2 の書き込み失敗ケースを除く)。
- **通常フレームで join / blocking しない**: production で `shutdown_and_join` /
  `close_and_drain` を呼ぶのは `src/lib.rs:1510, 1513` のみ。`App` に `Drop` impl は無く、
  `CollectionStoreRuntime::Drop` (`src/collection_store/runtime.rs:433-442`) は未完了 handle を
  detach する。唯一の blocking は C-9 の exit 経路のみ。
- **settings family との分離**: `settings_full_reset_does_not_change_collection_family_or_runtime_snapshot`
  (`src/collection_store/tests.rs:896`) が `collection.db` / `-wal` / `-shm` のバイト不変と
  actor snapshot 不変を固定。
- **`ConfirmRemove` / `ConfirmDelete` の stale 耐性**: 確認 modal は `expected_revision` +
  stable entry ID を握って送るので、開いている間に revision が進んでも `Conflict` になるだけで
  誤った行を消さない (`src/ui_dialogs/collections.rs:3041-3078`)。

---

## 項目11 — テスト対応表と欠落

全件棚卸しの結果、コレクション関連のテストは計およそ 130 件。
`src/collection_store/{db,model,path,text}.rs` に inline test mod は無く、検証は
`src/collection_store/tests.rs` に集約されている。

### 固定されている代表 (抜粋)

| 監査項目 | テスト (file:line) |
| --- | --- |
| 1 永続層: revision / manual 順 / 再オープン | `database_keeps_ids_manual_order_and_revisions_across_reopen` (collection_store/tests.rs:118) |
| 1 永続層: cascade / position 詰め直し / Conflict | `database_mutation_lifecycle_preserves_ids_compacts_positions_and_cascades` (tests.rs:238) |
| 1 永続層: no-op / Conflict で revision 不変 | `no_op_and_conflict_do_not_advance_revision` (tests.rs:397) |
| 1 永続層: 未知 schema を空で置換しない | `newer_schema_is_not_replaced_with_an_empty_catalog` (tests.rs:942) |
| 1 永続層: settings reset から独立 | `settings_full_reset_does_not_change_collection_family_or_runtime_snapshot` (tests.rs:896) |
| 2 path key: 別綴りの畳み込み | `source_key_collapses_legal_extended_drive_and_unc_spelling` (tests.rs:19) |
| 2 path key: external text admission | `external_text_policy_rejects_device_verbatim_and_root_escape` (tests.rs:41) |
| 2 path key: delete scope の component 境界 | `source_scope_exact_and_tree_use_component_boundaries` (collection_grid.rs:3207) |
| 3 actor: fanout / shutdown / panic / 起動失敗 | tests.rs:743, 791, 824, 851, 874 |
| 3 actor: Remote producer drain 順序 | `remote_producer_close_wakes_every_lease_and_drains_before_actor_shutdown` (runtime.rs:362) |
| 4 stamp: wanted で stale 化 / child は継続 | `root_load_owner_stales_on_wanted_revision_but_physical_continuation_does_not` (collection_grid.rs:2446) |
| 4 stamp: parked context への routing | collection_grid.rs:3067, 3133, 3014 |
| 4 stamp: 別 collection delete が現在を退役させない | collection_grid.rs:2858 |
| 4 stamp: intent sequence の ABA 拒否 | collection_navigation.rs:3914, 3957 |
| 5 reducer: entry ID→source key→head / tail | `latest_next_uses_current_prepared_facts_and_id_source_head_anchor_policy` (tests.rs:614) |
| 5 reducer: 見開き display unit / anchor 種別 | prepare.rs:819 |
| 5 reducer: preflight 有限性 / cancel | collection_navigation.rs:3559, 3606 |
| 5 reducer: Ctrl+↑↓ 連打集約 / 本の子 DFS | collection_navigation.rs:3623, 4201, 4283 |
| 6 child owner: 戻り先 / 履歴 / 失敗時維持 | collection_grid.rs:1713, 1811, 1837, 2051, 2406 |
| 6 child owner: 削除済み履歴の prune | `ready_catalog_prunes_deleted_collection_history_and_snapshot_rollback_cannot_restore_it` (collections.rs:4195) |
| 7 削除 fail closed | collections.rs:4033, 4155 / collection_grid.rs:1914 |
| 7 Shell submenu の分離 | context_menu.rs:3358 / native_context_menu.rs:2061 / context_menu_model.rs:2400 |
| 8 import/export: 純 parser / 往復 / atomic write | tests.rs:75 / prepare.rs:919, 964, 1002, 1056, 1101 |
| 8 import/export: 確認後だけ commit | collections.rs:4689, 5838 |
| 9 migration: 全 collection 単一 transaction / swap / rollback | tests.rs:417, 494 |
| 9 migration: 本 rename が root + ページを 1 tree | `completed_book_rename_migrates_root_and_pages_as_one_collection_tree` (app/tests.rs:23220) |
| 9 migration: journal ACK まで退役しない | app/tests.rs:23324, 52437 / collection_grid.rs:3217 |
| 9 migration: 即時終了が未読 journal を潰さない | `immediate_exit_merges_an_unloaded_recovery_journal_before_persisting` (app/tests.rs:23376 付近) |
| 10 manual order: group drag / Conflict 保持 / dirty close | collections.rs:3954, 5081, 5213, 4873 |
| 12 UI snapshot | collections.rs:6083, 6092, 6101, 6106 / tests/ui_snapshot.rs:393, 404 |

### 欠けている回帰テスト (本担当の指摘に対応するもの)

| # | 欠落 | 対応 | 追加すべき形 |
| --- | --- | --- | --- |
| T-1 | **`poll_rename_pending` を通した scope 決定** (file → Exact / folder → Tree) | **M-1** | handler-level。既存の `src/app/tests.rs:52376` は `spawn_rename_key_migration` を直接呼んで**壊れた配線を迂回**している |
| T-2 | ジャーナル read 失敗を「空」と区別する | M-2 | `journal_load` に読めないファイルを与え、その後の persist が既存ジャーナルを消さないこと |
| T-3 | actor `Busy` が UI にどう見えるか (Grid / 管理 UI 双方) | C-2, C-3 | queue を埋めて `open_collection_grid` → 一過性である (次フレームで再要求される) こと |
| T-4 | 並べ替え保存中の read 経路 | C-1 | `reorder.phase = Saving` のまま `start_collection_manual_navigation` → 「末尾」扱いにならないこと |
| T-5 | prepare 完了 → install の間に別 collection を開いた場合の拒否 | 監査項目4 | `Preparing` を組み立てて **結果を送る**。既存の collection_grid.rs:2896 と top_level_grid_view.rs:1154 は `Preparing` を作るところまでで結果を流していない |
| T-6 | prepare 中に notice が来て `exact_revision < wanted_revision` になった結果を捨てる分岐 (collection_grid.rs:1224) | 監査項目4 | 同上 |
| T-7 | notice 不在 → `Deleted` 断定の境界 | C-4 | 古い catalog_revision の notice を注入して `Deleted` にならないこと |
| T-8 | DB 破損 (ゴミバイト / NotADatabase) | K-1 | TempDir に非 SQLite を置いて `Failed` + ファイル不変 |
| T-9 | 3 件以上から中間を remove した後の manual_position 連続性と、その後の add_batch 位置 | C-10 | 既存 tests.rs:238 は 2→1 のみ。位置を assert するテストは他に無い |
| T-10 | text parser の Err 分岐 (`unquote_whole_line` の 4 分岐) / CR 単独 / 全角空白 / 極端に長い行 | C-6, C-7, C-8 | 純関数なので低コスト。現状 4 Err はどれも 1 件も駆動されていない |
| T-11 | export ファイルの先頭バイト (BOM 無し) | K-8 | 現在は `serialize_collection_paths` の戻り値が `"` 始まりであることから間接的に含意されるのみ |
| T-12 | subscriber の刈り取り | C-5 | mutation 無しで N 回 subscribe → `hub.subscribers` が伸び続けないこと |
| T-13 | Backward × Loop | K-6 | 現状 0 件。要素 1 件のコレクションを使ったナビゲーションテストも存在しない |
| T-14 | 終了時 migration 待ちの deadline | C-9 | actor を barrier で止めたまま exit → 有界時間で journal に回ること |
| T-15 | ジャーナル保存の恒久失敗 (予算枯渇後) | M-3 | 停止が 1 回だけ通知されること、および 100ms 再描画が止まること |
| T-16 | 決定的 `DuplicateSource` の毎起動リトライ | M-4 | 2 回目の boot で terminal になること |

---

## 本担当では判断していない点 / 未確認

- **Remote (`src/remote_ipc/persistent_collections.rs`、`crates/remote-web`) は別担当**。
  本担当が触れたのは (a) core actor と Remote の終了順 (`src/lib.rs:1499-1515`、plan §4.2 どおりで
  問題なしと判断)、(b) Remote が raw client を持たないこと、(c) Remote の request 頻度が
  C-2 / C-5 の実害規模に効くこと、の 3 点のみ。
- **`src/rename_key_migration.rs` の journal 内部 (bounded retry / atomic 置換 / writer thread) は
  並行監査の結果を取り込んだ**。M-1〜M-9 はその結果のうち、私自身がコードで再確認できたものだけを
  採用している (M-1 は `src/ui_dialogs/rename_item.rs:46/69/117/132/202` と全 grep、
  M-2 は `src/rename_key_migration.rs:235-268`、M-3 は `src/app.rs:31406-31421/31984-31986`、
  M-5 は `src/books.rs:174-178`、M-7 は `src/rename_key_migration.rs:270-277` を直接確認)。
  M-4 / M-6 / M-8 / M-9 は該当行の存在までを確認し、到達性は確定していない。
- **K-1 (バックアップ無し)、K-2 (自動保存)、K-5 (同名許可)、K-8 (BOM)** は仕様判断であり、
  利用者の意図を確認せずに「不具合」と断じていない。
- **実行時の挙動については、私自身の観測は一切無い**。上記はすべて source inspection のみ。
