# 再レビュー: 参照解除メニューの非対称 / 元の場所へ移動 / rename scope・journal 保護

対象: コミット済み `a5a1fc43b` (HEAD) 時点。`3d8f5d279` (A-3)、`8fbc4543d` (A-6 + メニュー世代化)、
`2a7167934` (M-1 / M-2) の差分と HEAD 全文を読んだ。
**アプリは起動していない。以下はすべてコードを読んだ限りの推定**であり、実行時の観測ではない。
作業ツリーの未コミット差分 (KeyAction 追加) には触れていない。行番号は HEAD のもの。

---

## 判定表

| ID | 判定 | 根拠 (要旨) |
| --- | --- | --- |
| A-3 | **解消** | `CollectionReferenceAvailability` を typed 化し、`Unavailable(reason)` で `RemoveFromCollection` を `enabled:false` + `disabled_reason` として残す。Grid の**単一選択** (`src/context_menu_model.rs:1481-1488`) と**チェック複数選択** (`:1179-1185`) の両方が同じ `remove_node()` (`:897-912`) を通る。native メニューは無効理由を tooltip で出す (`src/native_context_menu.rs:526-531`, `:1111`) |
| C-11 | **部分的** | 「安全な方だけ消える」非対称は解消。ただし Unavailable 時に**押せるのは破壊的操作だけ**という状態は残る (C 報告の修正案 = `MoveToRecycleBin` も無効化、は不採用)。Codex の「`delete_targets` は別対象を所有する」という判断自体は成立 (後述「問題なし」①)。残リスクは RB-2 |
| A-6 | **解消** | root を成功まで退役しない設計が成立。UI スレッドで `read_dir`/`metadata`/`canonicalize` を呼ばない。開始時・ready 時に surface/context/revision/entry/source/items generation を再検証。失敗・取消・stale・scope 拒否・置換で root 保持。成功時のみ `OpenRequestOwner::Navigation` で採用し Collection→Path 履歴が 1 本積まれる。周辺の穴は RB-4〜RB-6 |
| メニュー owner 世代化 | **解消** | `GridContextMenuOwner { index, context_id, items_generation }` (`src/ui_dialogs/context_menu.rs:76-82`)。`show_context_menu` は 1 箇所のみ (`src/app.rs:40xxx` 経由でなく `src/app.rs:74510`) から毎フレーム呼ばれ、世代不一致で閉じる。通常フォルダで世代が動くのは全件差し替え時だけ (`bump_items_generation` の呼び元 = `start_loading_items` / `remove_items_batch` / smart folder / snapshot ops)。サムネ更新や watch の**内容不変**な再走査では動かない (`apply_external_rescan` は `current_folder_signature` 一致でスキップ、`src/app.rs:20733-20738`)。過剰失効ではない。回帰テストの穴は RB-11 |
| M-1 | **解消** | `RenamePending { rx, collection_scope }` (`src/ui_dialogs/rename_item.rs:13-16`) が開始時に scope を所有。`start_rename_item_after_confirmation` (`:127-150`) が `clear_rename_dialog_state()` の**前**に scope を確定し、`poll_rename_pending` (`:152-`) は `pending.collection_scope` を使う。dialog 消去・Empty poll・abort・error のいずれでも失われない。file→Exact / folder→Tree も同関数で決まる。残は RB-7 |
| M-2 | **部分的** | read/parse/不在の 3 状態を typed 区別 (`src/rename_key_migration.rs:233-268`)。読取失敗時は旧 bytes を削除も上書きもしない (`persist_rename_migration_journal_attempt` 冒頭の gate)。物理変更前 admission は **rename / delete / 本棚 rename・delete・reorder flush・transfer(Move/Copy)** の全入口に入っている。**ただし恒久的な parse 失敗から抜ける手段がアプリ内に無く、削除まで恒久的に不能になる (RB-1, P1)** |
| M-3 | **未対応** | `poll_rename_migration_pending` 末尾の repaint 条件は queue 非空で無条件 100ms のまま (`src/app.rs:32858-32866`)。保存恒久失敗時の 10Hz は残る。§23.11 の延期対象として整合 |
| M-4 | **未対応** | `DuplicateSource` / `InvalidPath` は従来どおり `rename_migration_boot_retry` へ。延期対象 |
| M-7 | **未対応** | `journal_save` は `std::fs::write(&tmp, &json)` のあと `sync_all` 無しで置換 (`src/rename_key_migration.rs:275-295`)。**この未対応が RB-1 の前提条件 (切断 journal) を作る** |

---

## 新規指摘

### RB-1 (P1) 恒久的に解析不能な journal で、名前変更・**削除**・本棚のパス操作が恒久的に不能になる。アプリ内に復旧手段が無い

**根拠**
- `src/rename_key_migration.rs:250-268` — `journal_load` は `NotFound` 以外の read 失敗と、現行/legacy どちらにもパースできない内容を `Err` で返す。
- `src/app.rs:32242-32258` — 初回 `Unloaded` の読み取りが `Err` なら `RecoveryFailed { message, .. }` へ落ちる。**`Durable` からこの状態へ戻ることはないが、`RecoveryFailed` から `Durable` へ戻れるのは「再読込が成功したとき」だけ**。
- `src/app.rs:32357-32385` — `admit_rename_migration_source_change` は `RecoveryFailed` のとき worker で再読込するだけ。**壊れたファイルの隔離・退避・破棄は一切しない**。`JOURNAL_FILE` を消す / 退避するコードはリポジトリに無い (grep 済み。`journal_save` の空削除だけで、それは gate で止まる)。
- gate される入口 = `src/ui_dialogs/rename_item.rs:135`、`src/app.rs:33124` (`start_delete_files`)、`:35322` / `:35335` / `:35384` / `:35417` (本棚 rename / delete / reorder flush / transfer)。
- `start_delete_files` は **Delete キー / 右クリック / チェック一括のすべての削除の単一入口** (`src/ui_dialogs/context_menu.rs:2072`, `:2126`)。コレクションを使っていない利用者にも等しく効く。

**失敗シナリオ (推定)**
1. 名前変更 → migration 未完のまま電源断。`journal_save` は `sync_all` しない (M-7 未対応) ので、名前は正しいが中身が途中/空の `rename_migration_journal.json` が残り得る。
2. 次回起動、`journal_load` が `Parse` エラー。`RecoveryFailed`。
3. 以後、**そのセッションだけでなく毎セッション**、削除・名前変更・本棚の移動/並べ替え/転送が
   「復旧記録を読み取れないため変更を保留しました (parse failed: …)」で拒否され続ける。
   parse は決定的なので再読込は毎回失敗する。
4. 終了のたびに blocking の警告ダイアログ (RB-9) が出る。利用者は**どのファイルをどうすればよいか
   知らされない** (文言にパスが無い。RB-8)。

修正前 (`2a7167934` 以前) は、壊れた journal はログ 1 行で破棄され、失うのは未完了 migration だけで
アプリの操作性は保たれていた。M-2 は「旧 bytes を守る」ために**操作不能と引き換えにした**形になっている。

**修正方向**
`Read` (一過性でありうる) と `Parse` (決定的) を admission でも区別する。`Parse` は
`rename_migration_journal.corrupt-<timestamp>.json` へ **rename で退避** (削除しない) してから空 journal として
続行し、「引き継げなかった可能性がある」旨を 1 回通知する。リポジトリには `settings_db` の quarantine
という前例がある (CLAUDE.md「settings.db ... quarantine」)。`Read` 失敗は現行の保留 + 明示再読込のままでよいが、
利用者向け文言にファイルのフルパスと対処 (別名に退避して再起動) を入れる。

---

### RB-2 (P3) 参照解除が Unavailable の窓で、押せるのは破壊的操作だけ。削除確認スキップ設定と重なると無確認で元ファイルが消える

**根拠**
- `src/context_menu_model.rs:1490-1505` — `MoveToRecycleBin` は `kind.supports_delete()` だけで **enabled** のまま出る。
- `src/ui_dialogs/context_menu.rs:533-537` — `should_skip_delete_confirmation` は `skip_recycle_bin_delete_confirmation && kind == RecycleBin` で確認ダイアログを丸ごと飛ばす。
- 同 `:497-531` — 確認ラベルは「「name」をゴミ箱に移動しますか？」で、**コレクション参照の元ファイルである旨を含まない** (メニューのラベルだけが「元ファイルを…」と言っている)。

**失敗シナリオ (推定)** コレクション直下で項目を追加/削除した直後 (再 prepare 中) に別セルを右クリック →
「コレクションから外す」は灰色、押せるのは「元ファイルをゴミ箱へ移動 (タグ・評価も整理)」だけ →
確認スキップを ON にしている利用者は 1 クリックで元ファイルを失う。既定は OFF (`src/settings.rs:6863`) なので P3。

**修正方向** 最小は、`collection_reference.is_collection_root()` のとき `should_skip_delete_confirmation` を
無効化する (確認を必ず出す)。あわせて確認ラベルに「コレクションの元ファイル」であることを入れる。
C-11 案 (同じ理由で `MoveToRecycleBin` も無効化) を採らない判断自体は「①」のとおり支持できるので、
危険側を殺すのではなく確認を強制する方向を推す。

---

### RB-3 (P3) 右クリックメニュー編集で `RemoveFromCollection` を非表示にでき、その場合は破壊的項目だけが残る

**根拠** `src/context_menu_model.rs:1087-1090` の `units.retain(|unit| settings.is_visible(item))`、
`:311-315` の `is_visible` は `hidden_items` の opt-out。`RemoveFromCollection` は
`ContextMenuItemId::ALL` に含まれる (`:105`) ので利用者が隠せる。
新規 ID は既定で可視なので v3.x からの移行では消えない (確認済み)。

**評価** 利用者の明示的な選択であり、Delete キーは fail closed のまま (`src/ui_dialogs/context_menu.rs:2207-2217`)
なので安全性の破れではない。ただし「安全な選択肢が常に見える」という A-3 の保証は絶対ではない。
情報として記録し、対処するなら「コレクション直下では隠していても表示する」か、設定 UI 側で注意書きを出す。

---

### RB-4 (P3) 「見つかりません」の項目から「元の場所へ移動」が出ない

**根拠**
- `src/context_menu_model.rs:1357-1367` の kind 許可リストに `ContextMenuItemKind::CollectionPlaceholder` が無い。
- `src/grid_item.rs:773` — `CollectionPlaceholder` の `drag_source_path()` は `None`。`jump_to_folder_request` の
  `_` アーム (`src/ui_dialogs/context_menu.rs:108-116`) も `None` を返す。

**失敗シナリオ (推定)** 元ファイルが消えた/移動した entry こそ「元あった場所を見に行きたい」動線だが、
メニューに項目が出ない (誤った理由文言も出ない点は安全)。
`GridItem::CollectionPlaceholder { path, .. }` は path を持っているので、親フォルダへの移動は技術的に可能。

**修正方向** placeholder を kind リストへ加え、`context_jump_request` で `path` の親を destination、
selection は `None` (exact 不在通知は既存の「移動先の項目が見つかりません」で足りる)。

---

### RB-5 (P3) ドライブ直下 / UNC 共有ルートを登録したフォルダ項目で、無効理由が事実と違う

**根拠**
- `src/collection_store/path.rs:231-251` — `components.is_empty()` のとき `display = root`。`D:\` や `\\server\share`
  そのものが正当な登録 path として通る。
- `src/ui_dialogs/context_menu.rs:2699-2701` — `parent_folder_for_nav` は `path.parent()?`。上記 2 つは `None`。
- `src/ui_dialogs/context_menu.rs:1844-1848` — `Ready` なのに `context_jump_request` が `None` なので、
  無効理由が **「コレクション一覧を更新中です」** になる。待っても永久に変わらない。

**修正方向** 親が取れない場合の typed 理由を分け (「この項目に親フォルダはありません」)、
`dispatch_context_jump_request` の「もう一度選択してください」トーストもこのケースには出さない。

---

### RB-6 (P3) フルスクリーンだけ検索と非対称 — 検索結果は FS でも「フォルダに移動」が出るが、コレクションは出ない

**根拠** `src/context_menu_model.rs:1353-1367` — `can_jump_to_folder` の `in_search` 枝には
surface 制約が無く、Fullscreen でも出る。一方 `collection_root_grid` は `surface == Grid` を要求する。

**評価** 「Grid 専用入口にする」という §23.6A の判断は、FS の viewer session を root 退役と同時に
畳む複雑さを避けるという意味で理解できる。ただし**既存機能との非対称**なので、意図であるなら
`docs/collection-implementation-plan.md` §23.6A に「検索 FS との差を承知で Grid 限定にした理由」を 1 行足すべき。
`RemoveFromCollection` / `MoveToRecycleBin` はどちらも Grid 限定なので、FS の**削除系**は非対称になっていない (問題なし)。

---

### RB-7 (P3) M-1 の「捕捉側」に肯定的テストが無く、scope が失敗しうる `Path::is_file()` 由来で Tree へ fail-open する

**根拠**
- `src/ui_dialogs/rename_item.rs:50` — `self.rename_target_is_file = target.is_file();`。stat に失敗した実ファイル
  (権限・一時的な切断) は `false` → `Tree` になる。M-1 の元の失敗シナリオ (prefix 巻き込み書き換え) と同じ向き。
- テスト: `rename_pending_uses_captured_scope_after_empty_poll_and_dialog_clear` (`src/app/tests.rs:23983`) と
  `rename_pending_tree_scope_and_abort_error_consume_request_once` (`:24012`) は **`RenamePending` を直接構築**する。
  `start_rename_item_after_confirmation` を通るテストは `unreadable_rename_journal_blocks_every_owned_mutation_before_worker_start`
  (`:23801`, `:23941`) だけで、そこは **admission が false の経路**なので scope 決定に到達しない。
  → 「`target_is_file` → `Exact`/`Tree`」の対応を固定したテストが 1 件も無い。

**評価** 壊れていた配線 (dialog clear → poll) は確かに固定されたので M-1 は解消。ただし
「テストが配線を迂回する」という元指摘の構図が、1 段上 (capture 側) に残っている。

**修正方向** admission Durable で `start_rename_item_after_confirmation(.., target_is_file=true/false)` を呼び、
`rename_pending.collection_scope` を assert する 1 件を足す。あわせて scope を `GridItem` の kind
(`request_grid_rename_dialog` は item を持っている) から導ければ、stat 失敗の fail-open が構造的に消える。

---

### RB-8 (P3) M-2 の利用者向け文言に serde の英語メッセージが混ざり、ファイルの場所も対処も書かれていない

**根拠**
- `src/app.rs:32364`, `:32411-32419` — トーストが `format!("... ({failure})")` で
  `JournalLoadError::Display` (`src/rename_key_migration.rs:238-245`) をそのまま埋め込む。
  実体は `parse failed: expected value at line 1 column 1` のような英語。
- `src/app.rs:32192-32202` — 終了時の警告も「次回起動後に読み取れる状態へ戻してください」だけで、
  `%APPDATA%\mimageviewer\rename_migration_journal.json` を指していない。
- 元レビュー C-8 が同型 (「失敗文言に std の英語メッセージが出る」) を既に指摘している。

**修正方向** 英語 detail はログへ、利用者にはファイル名/場所と「別名に退避して再起動」を日本語で示す。
RB-1 の quarantine を入れるなら、この文言は「退避しました」に置き換わる。

---

### RB-9 (P3) 終了時に blocking の MessageBox が出る (毎セッション)

**根拠** `src/app.rs:32195-32202` — `resolve_collection_migration_for_exit` が recovery 失敗時に
`#[cfg(not(test))] crate::native_name_dialog::show_warning(...)` を呼ぶ。呼び元は `on_exit` (`src/app.rs:75906`)。

**評価** Windows のログオフ/シャットダウン時に modal を出すと OS 側の終了を妨げる (最悪 kill される)。
また、利用者が何も名前変更していなくても、journal が読めない限り**毎回**出る。
トレイ常駐の Exit でも出る。トースト/ログに落とすか、RB-1 の quarantine で状態自体を解消するのが筋。

**補足 (元 C-9 / B-19)**: 終了時の `rx.recv()` は `src/app.rs:32213` に**無期限のまま**残っている。
M-2 で悪化はしていない (recovery 失敗時は take する前に早期 return するので、そもそも in-flight が存在し得ない —
in-flight は `Durable` でしか始まらず、`Durable` から `RecoveryFailed` へ戻る遷移は無いため)。改善もしていない。

---

### RB-10 (P3) A-6 の `sidecar_restore_active()` 分岐が無言で何も起きない

**根拠** `src/app.rs:40149-40151` (開始時) と `:40254-40256` (ready 時) が `return` / `return None` するだけで
トーストもログも無い。利用者からは「『元の場所へ移動』を押したのに何も起きない」に見える。
他の拒否経路 (stale、scope 拒否) はトーストを出しているので、ここだけ非対称。

---

### RB-11 (P3) メニュー世代 owner の回帰テストが collection 経路だけ

**根拠** `grid_menu_owner_rejects_replaced_items_and_collection_install_preserves_sibling_menu`
(`src/app/collection_grid.rs:3553`) は `install_collection_grid_items` しか駆動しない。
**通常フォルダで外部変更 → `apply_external_rescan` → `load_folder_with_scan` → 世代 bump** のときに
開いていたメニューが閉じる、という新しい挙動を固定したテストが無い。
コード読みの限りでは「内容が実際に変わったときだけ」閉じるので退行ではないが、
将来 `bump_items_generation` の呼び元が増えたときに気づけない。

**修正方向** 通常フォルダで `capture_grid_context_menu_owner` → `bump_items_generation` →
`show_context_menu` が `None` を返し `context_menu_idx` が落ちることを 1 件固定する
(= 世代が動く条件が広がったら落ちるテスト)。

---

## 確認して問題なしと判断した項目

① **`delete_targets` が別項目を指すことは無い**。`src/ui_dialogs/context_menu.rs:1273-1281` で
   `delete_targets` は右クリック時点の `item.drag_source_path()` から **path として**焼き付けられ、
   `NativeGridContextMenuTarget` にコピーで保持される。native メニューは
   `show_native_context_menu` (`:1191`) が TrackPopupMenu ベースの blocking 呼び出しで、
   戻ってきた後の dispatch も同じ捕捉済み `&target` を使う (`:1787-1798`)。
   egui fallback でも `target` は毎フレーム再構築され、世代が動けばその前に `show_context_menu` (`:885-892`) が
   メニューを閉じる。よって「メニューを開いた時点の `delete_targets` が別項目を指す」経路は見つからなかった。
   Codex の「参照解除とは別の対象を所有する操作」という判断は成立している。
② **チェック複数選択のメニューに「元の場所へ移動」は出ない**。`has_checked` 枝は
   `src/context_menu_model.rs:1260` で早期 return するため。検索の Jump と同じ「単一項目のみ」契約で一貫。
③ **背景 (空き領域) 右クリック**でコレクション用の項目が漏れない。`is_folder_context` 時は
   `RemoveFromCollection` / `MoveToRecycleBin` / `JumpToFolder` がいずれも `!is_folder_context` 条件で出ない。
   そもそもコレクション直下は `current_folder == None` なので `show_context_menu` が `:901-903` で閉じる。
④ **フルスクリーンの削除系に非対称は無い**。`RemoveFromCollection` も `MoveToRecycleBin` も Grid 限定。
⑤ **既存の Search / 閲覧履歴 Jump は不変**。`context_jump_request` の `NotCollectionRoot` アーム
   (`src/ui_dialogs/context_menu.rs:1868`) が `jump_to_folder_request` の結果を素通しし、
   `begin_context_jump_to_folder` も `origin.is_none()` なら従来の `dismiss_source_for_jump_to_folder` 経路へ入る
   (`src/app.rs:40170`)。origin 付きのときだけ dismiss を飛ばす。
⑥ **ZIP / PDF entry の「元の場所」は書庫のある親フォルダ**。`drag_source_path()` が ZIP/PDF 本体を返し、
   `context_jump_request` が `parent_folder_for_nav` でその親へ落とす。コレクション直下に `ZipImage` /
   `PdfPage` は出ない (entry の kind が実ファイル種別のみ) ので、書庫内ページへの誤着地は起きない。
⑦ **Folder セルも親へ移動して自分を選択する**。`context_jump_request` が `jump_to_folder_request` の
   Folder アーム (フォルダ自身へ移動) を上書きする (`src/ui_dialogs/context_menu.rs:1878-1886`)。
   「元の場所」= 登録元の置き場所、という語義と一致。
⑧ **A-6 の UI スレッド I/O は無い**。開始時に呼ぶのは `path.parent()` と `snapshot_scope_allows_open` /
   `sidecar_restore_active` (いずれも in-memory)。走査は `start_folder_open_scan` の worker
   (`folder_pane_open_pending { cancel, rx }`)。ready 側も `ScannedDir` を受け取るだけ。
⑨ **A-6 の stamp 再検証は開始時と ready 時の両方**。`collection_jump_origin_is_current` (`src/app.rs:40218-40239`)
   が origin 種別 (Root)・`accepted_revision == wanted_revision`・detached 述語・source/parent の path_eq・
   `collection_grid_physical_load_owner_is_current` (entry_id + source_key + source_path + position +
   wanted_revision + installed/current items_generation + stamp) を全部通す。
   `begin_context_jump_to_folder` と `apply_jump_to_physical_folder_ready` の両方から呼ばれる。
⑩ **root 保持と履歴**。`collection_location_jump_preserves_root_until_success_and_selects_exact_source`
   (`src/app/collection_grid.rs:3328`) が items_generation / selected / scroll_offset_y / address / surface の
   保持、取消・置換での保持、成功後の `FolderNavHistoryTarget::Collection` back target、
   兄弟 quick-folder 履歴の非干渉を固定している。
⑪ **元項目が消えていても親フォルダへは移動し、既存の exact 不在通知を出す**
   (`collection_location_jump_scan_failure_keeps_root_and_missing_leaf_reports_exact_miss`, `:3494`)。
   別項目を誤選択しないことも assert 済み。
⑫ **M-2 の admission は「全 mIV 所有の物理変更入口」を網羅している**。削除は `start_delete_files` が
   単一入口 (Delete キー / 右クリック / 一括 / 確認スキップのすべてが通る)。名前変更は
   `start_rename_item_after_confirmation` が単一入口。本棚は rename / delete / reorder flush / transfer(Move,Copy)。
   append / create / list / 閲覧は非 gate (意図どおり)。
   **未 gate で問題ないと判断したもの**: 切り取り→貼り付け・D&D による移動 (mIV は移動に対する
   path-key migration を enqueue していない。`spawn_rename_key_migration` / `enqueue_collection_source_migration_batch`
   の production 呼び元は rename と本棚ページ移動の 2 つだけ)、アーカイブ変換 (新規ファイル生成)、
   メタ操作の Undo (`undo_stack` は rating/tag のみ)。
⑬ **`journal_load` の `Err` を空扱いする箇所は他に無い**。production の呼び元は
   `src/app.rs:32247` (初回) と `:32370` (再読込 worker) の 2 つだけで、両方 typed に扱う。
⑭ **通常時に余計な I/O / 遅延が増えていない**。`try_start_next_rename_migration` は
   queue が空なら `rename_migration_journal_allows_start()` の前に return する (`src/app.rs:32563-32568`) ので、
   毎フレームの `ensure_rename_migration_journal_loaded` 経由の I/O は起きない。
   初回の同期 read は従来どおり 1 回 (`poll_rename_migration_pending` 冒頭)。
   `RecoveryFailed` 状態では `matches!` 判定だけで I/O は無い。
⑮ **再読込中に idle が空転しない**。`poll_rename_migration_pending` 冒頭の
   `ensure_rename_migration_journal_loaded()` が毎フレーム `poll_rename_migration_recovery_retry()` を呼ぶので、
   queue が空でも worker 結果は次フレームで消費される。`RecoveryRetrying` の間だけ 100ms 再描画を予約し、
   `RecoveryFailed` / `Durable` では予約しない (`src/app.rs:32858-32866`)。
   `empty_recovery_retry_is_polled_without_migration_work` (`src/app/tests.rs:23917`) が固定。
⑯ **成功済みの遅延結果と deferred delete が失われない**。`RecoveryFailed` / `RecoveryRetrying` の間、
   `invalidate_rename_migrations_for_removed_paths` は `deferred_removed` へ退避し (`src/app.rs:31898-31907` 相当)、
   再読込成功時に FIFO 統合後へ適用する。`explicit_retry_merges_old_jobs_and_deferred_delete_before_save_ack`
   (`src/app/tests.rs:23869`) と `late_migration_completion_and_exit_preserve_unreadable_prior_bytes` (`:23947`) が固定。
⑰ **本棚削除の invalidation が worker の実 path で行われる**。`BookOpResult::Deleted { name, path }`
   (`src/books.rs:86-90`, `:750-753`) と `BookOpIntent::Delete { path }` の照合 →
   `book_delete_completion_invalidates_by_worker_success_path` (`src/app/tests.rs:24054`)。
   設定から再構成した path ではない。
⑱ **物理変更の in-flight 中に generic migration が始まらない**。
   `rename_migration_source_changes_pending` (`src/app.rs:2542-2560` 相当、HEAD `:32542`) が
   `delete_pending` と `BookOpIntent::blocks_rename_migration()` を見る →
   `pending_delete_and_book_path_workers_hold_generic_migration_start` (`src/app/tests.rs:24088`)。

---

## テスト対応表

| 対象 | テスト | 位置 (HEAD) | 配線を通るか |
| --- | --- | --- | --- |
| A-3 model | `unavailable_collection_reference_keeps_disabled_remove_with_reason` | `src/context_menu_model.rs:2533` | 単一/複数の両方をループ。model 層 |
| A-3 handler | `unavailable_collection_reference_is_visible_with_reason_but_physical_delete_remains_separate` | `src/ui_dialogs/context_menu.rs:3575` | `app.context_menu_nodes(&target, ..)` を通る |
| A-3 順序 | `collection_root_lists_reference_remove_before_explicit_source_delete` | `src/context_menu_model.rs:2450` | model 層 |
| A-6 model | `collection_root_location_jump_is_scoped_and_disabled_when_binding_is_unavailable` | `src/context_menu_model.rs:2488` | model 層。FS に出ないことも assert |
| A-6 handler | `collection_jump_menu_projects_source_availability_without_disabling_reference_remove` | `src/ui_dialogs/context_menu.rs:2917` | `context_menu_nodes` + `dispatch_context_jump_request` |
| A-6 保持/成功 | `collection_location_jump_preserves_root_until_success_and_selects_exact_source` | `src/app/collection_grid.rs:3328` | `begin_context_jump_to_folder` → pending → `apply_..._ready` |
| A-6 stale | `collection_location_jump_rejects_stale_revision_items_and_context` | `src/app/collection_grid.rs:3416` | revision / items / context の 3 種 |
| A-6 失敗/不在 | `collection_location_jump_scan_failure_keeps_root_and_missing_leaf_reports_exact_miss` | `src/app/collection_grid.rs:3494` | `resolve_main_folder_open_ready` を通る (`cfg(windows)`) |
| メニュー世代 | `grid_menu_owner_rejects_replaced_items_and_collection_install_preserves_sibling_menu` | `src/app/collection_grid.rs:3553` | collection install のみ (RB-11) |
| M-1 poll | `rename_pending_uses_captured_scope_after_empty_poll_and_dialog_clear` | `src/app/tests.rs:23983` | `poll_rename_pending` は通るが `RenamePending` は直接構築 (RB-7) |
| M-1 Tree/取消 | `rename_pending_tree_scope_and_abort_error_consume_request_once` | `src/app/tests.rs:24012` | 同上 |
| M-2 入口網羅 | `unreadable_rename_journal_blocks_every_owned_mutation_before_worker_start` | `src/app/tests.rs:23801` | rename / delete / 本棚 5 種を実関数で叩き、旧 bytes 不変を assert |
| M-2 再読込統合 | `explicit_retry_merges_old_jobs_and_deferred_delete_before_save_ack` | `src/app/tests.rs:23869` | `admit_` → worker → `ensure_` |
| M-2 空再読込 | `empty_recovery_retry_is_polled_without_migration_work` | `src/app/tests.rs:23917` | `poll_rename_migration_pending` を通る |
| M-2 遅延完了/終了 | `late_migration_completion_and_exit_preserve_unreadable_prior_bytes` | `src/app/tests.rs:23947` | `resolve_collection_migration_for_exit` を通る |
| M-2 load 区別 | `journal_load_distinguishes_read_error_and_legacy_pairs` / `journal_roundtrip_and_cleanup` | `src/rename_key_migration.rs:2358` / `:2324` | read/parse/legacy/不在 |
| M-2 本棚削除 | `book_delete_completion_invalidates_by_worker_success_path` | `src/app/tests.rs:24054` | `delete_book` の実結果を通す |
| M-2 in-flight 抑止 | `pending_delete_and_book_path_workers_hold_generic_migration_start` | `src/app/tests.rs:24088` | `try_start_next_rename_migration` |

**欠けている回帰**: RB-7 (capture 側 scope)、RB-11 (通常フォルダのメニュー世代)、
RB-1 を直すなら「決定的 parse 失敗を退避して操作を回復する」テスト、
RB-2 を直すなら「コレクション直下では削除確認をスキップしない」テスト。

---

## 出荷前に必要と考えるもの

1. **RB-1 (P1)** — 恒久 parse 失敗からの回復導線。これだけは出荷前に必要と考える
   (影響が「削除が永久にできない」で、コレクションを使わない利用者にも及ぶため)。
2. RB-2 / RB-5 / RB-8 は小さく、同じ chunk で片付けられる。
3. RB-4 / RB-6 / RB-7 / RB-9 / RB-10 / RB-11 は v4.0.x で可。
   ただし RB-6 は「意図した非対称」なら §23.6A に理由を 1 行残すだけで済む。
