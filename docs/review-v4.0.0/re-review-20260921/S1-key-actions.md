# S1: v4.0.0 コレクション 追加レビュー — キー操作 (§23.9 / §23.12)

作成: 2026-09-21 / 担当: ClaudeCode Opus 5 / 対象: 作業ツリーの未コミット差分 (HEAD = `a5a1fc43b`)

**前提**: 読み取り専用のコード照合のみ。アプリは起動しておらず、cargo も実行していない
(`python scripts/check_ui_glyphs.py` のみ実行 = exit 0)。**実行時の観測はゼロ**で、本書の
「〜になる」はすべてコードを読んだ限りの推定である。

## 0. 要約

| 重要度 | 件数 | ID |
| --- | --- | --- |
| P1 (出荷前必須) | 1 | S1-1 |
| P2 (出荷前推奨) | 6 | S1-2 〜 S1-7 |
| P3 | 8 | S1-8 〜 S1-15 |

**総評**: keymap 規約 (`ini_name` / `context` / `trigger` / `default_chords` / `ALL_ACTIONS` /
`docs/keymap.ini.default` / メニュー連携 / リング parity) は**機械的なテストで守られており、
欠落は見つからなかった**。所有境界 (source lease・要求 ID・押下時 capture・勝者だけが採用) も
設計どおりに実装されていて、モーダルの終了経路は追える限りすべて終端に到達する。
残る出荷前必須は 1 件で、キーの実装ではなく**「番号」が利用者から見えない**ことにある。

## 0.1 項目 8 — 再レビュー指摘との照合

| ID | 内容 | 作業ツリーでの状態 | 根拠 |
| --- | --- | --- | --- |
| RA-1 | リングピッカーのソート行が Collection root でも global `settings.sort_order` を書く | **未解消** (悪化なし) | `src/app/gamepad_input.rs:4908-4915` の `apply_grid_picker_sort_order` は surface を見ずに `settings.sort_order = sort_order; settings.save();`。今回の差分は同ファイル +23 行だが `RingActionId::AddToCollection` の dispatch (`:5786-5789`) と `apply_ring_add_to_collection` (`:6389-6407`) の追加のみ |
| RA-2 | 列ヘッダ無効理由の文言が事実と違う | **未解消** | `src/ui_main.rs:15844` の文言、`src/app.rs:52657` の `details_header_sort_locked()` はいずれも bool のまま。今回の差分は `ui_main.rs` に `AddressBarNav` 2 variant の match 追加とテスト追加のみ |
| RA-4 | 音楽ビュー HUD の前/次だけ `current_grid_order` | **未解消** | `src/ui_music_panels.rs:344` は `self.current_grid_order().to_vec()` のまま。同ファイルは今回の差分に含まれない |
| RC-1 | ナビゲーションの読み取りエラーが英語のまま | **未解消** | `src/app/collection_navigation.rs:984` は `&error.to_string()` のまま。同ファイルは今回の差分に含まれない |
| RE-1 | バックアップの成否が `eprintln!` のみ | **未解消** | `src/collection_store/db.rs:104, 128, 135` に `eprintln!` が残る |
| RE-5 | マニュアルが未コミットのキー操作を実装済みとして説明 | **解消見込み** (この差分が同時に入る前提) | 実装と記述を行単位で突き合わせた結果は §3。番号の意味・既定キー無し・前後の循環・管理画面を開く操作の記述はいずれも実装と一致。**ただし S1-1 の「番号が作成順で固定・削除で繰り上がる」点だけは未記載** |

**偶然直っていたものは無い。悪化しているものも無い。**

---

## 1. 指摘

### S1-1 (P1) コレクションの「番号 1〜20」が管理画面のどこにも出ず、削除すると黙って繰り上がる

**根拠**
- 番号の解決: `src/app/saved_group_actions.rs:98-103` — `ids.get(slot - 1)`。`ids` は
  `src/ui_dialogs/collections.rs:1750-1754` が `catalog.definitions` の順に作る。
- その順は `src/collection_store/db.rs:604, 702` の `ORDER BY catalog_position ASC`。
  `catalog_position` は `db.rs:200` の `COALESCE(MAX(catalog_position) + 1, 0)` = **作成順**。
- **並べ替え UI が無い** — `catalog_position` を更新するのは `db.rs:974-990` の
  `compact_catalog_positions` だけで、その唯一の呼び出しは `db.rs:253` の
  `delete_collection` 内。利用者が順番を指定する経路は存在しない。
- **管理画面に番号列が無い** — `src/ui_dialogs/collections.rs:3379-3420` の行は
  `●` (追加先マーカー) / 名前の TextEdit / 「開く」「削除」等のボタンだけ。
  比較対象: お気に入り編集は `src/ui_dialogs/favorites_editor.rs:518` に「番号」列、
  ピン留めタグ編集は `src/ui_dialogs/tag_editor.rs:74-75` に「番号」列 +
  「ピン留めタグ1〜20のコマンドで使う順番です」の hover を持つ。
- マニュアルの記述は `htdocs/mimageviewer/manual/tut-collections.html` (未コミット差分) の
  「番号は管理一覧の全件の順番で、ツールバーに固定した一覧だけを数えるものではありません。」
  のみ。**作成順であること・並べ替えられないこと・削除で繰り上がることは書かれていない。**

**想定される失敗 (推定)**
1. 利用者が A/B/C/D の 4 つを作り、`GridOpenCollection3` に F5 を割り当てる (= C を開く想定)。
   管理画面に番号が出ないので、3 番が C であることは「上から数える」しか確かめる手段がない。
2. B を削除すると `compact_catalog_positions` が走り、C は 2 番、D が 3 番になる。
   **キーの割り当ては変えていないのに F5 は D を開くようになる。** 画面のどこにも
   「番号が変わった」という手がかりが無い。
3. お気に入り (番号列あり・編集画面で並べ替え可) とピン留めタグ (番号列あり) では
   同じ事故は起きないので、コレクションだけが例外になる。

**修正方向 (小さい順)**
- 最小: 管理画面の行頭に番号列を出す (`favorites_editor.rs:518` と同じ形)。
  `is_target` の `●` の隣に `ui.monospace(format!("{:>2}", index + 1))` 程度。
  あわせて hover に「操作カスタマイズの『コレクション N を開く』で使う順番です」を添える。
- 同時に、マニュアルへ「番号は作成順で、削除すると以降が繰り上がります」を 1 行足す。
- 本棚側 (`GridOpenBook1..20`) は**名前順**なので管理画面を見れば数えられるが、
  改名で順番が動く点は同じ。番号列を出すなら両方に出すのが揃う。
- 並べ替え UI (初回レビュー F-5) は v4.0.x 送りでよいが、その場合こそ番号の可視化が要る。

**判断の注釈**: 「操作不能」の直接の形ではないため P2 との境界にある。P1 と置いたのは、
(a) 利用者が承認した仕様「管理画面の全件順の 1〜20 番」が、その全件順を利用者が読めない
状態では成立しないこと、(b) **削除で割り当ての意味が黙って変わる**のが「誤登録」に近い
実害であること、(c) お気に入り・タグに既にある番号列を足すだけで閉じること、による。

---

### S1-2 (P2) 待機モーダル表示中にトレイへ格納すると、モーダルが描かれないまま全ウィンドウのキーが止まる

**根拠**
- `saved_group_open_modal_visible()` (`src/app/saved_group_actions.rs:238-250`) が
  `modal_dialog_block_reason` (`src/app.rs:18765`) に入り、`common_modal_dialog_open()`
  → `any_dialog_open()` / `any_modal_dialog_open_for_fullscreen_keys()` →
  `keyboard_ownership_snapshot` の `modal` (`src/app.rs:18554-18558`) になる。
  `modal` が真だと `KeyboardOwner::Modal` → `blocks_legacy_main_shortcuts()` が真
  (`src/keyboard_input.rs:253-255`)。**root 以外の viewport (フルスクリーン / detached) も同じ。**
- 一方 `render_saved_group_open_modal` は `update_frame` (`src/app.rs:74446`) の
  メインウィンドウ側でしか描かれない。`egui::Modal` は OS のタイトルバーの `[×]` を
  塞がないので、トレイ常駐 ON で閉じるとモーダルは消えるが `self.saved_group_open` は残る。
- `poll_saved_group_open` は `WaitingBookList` / `ScanningBook` /
  `WaitingCollectionCatalog` / sidecar 待ちのいずれでも
  `ctx.request_repaint_after(100ms)` を出す (`saved_group_actions.rs:437, 455, 493, 513`)。
  **トレイ常駐中も 10Hz で起き続ける** — `docs/idle-health-check.md` の `tray-residency`
  で見る `max_update_rate` に抵触し得る。

**想定される失敗 (推定)**
コレクション actor が `Starting` のまま (= 初回起動直後や DB が遅い環境) に
`GridOpenCollection1` を押し、待機中に `[×]` でトレイへ格納する。以後、別窓 (detached) の
キーがすべて無反応になり、画面上に理由が出ない。トレイから復帰して「中止」を押すまで続く。

**注記**: 同じ形は既存の `staged_smart_root_modal_visible()` (`src/app/smart_folder.rs:1419`)
にもあるので、この差分が新しく作った欠陥ではない。ただしコレクションの `Starting` は
終端が保証されていない (S1-3) ので、到達しやすさが上がっている。

**修正方向**: (a) モーダル owner が居る間は close-to-tray を保留する、
(b) `window_visible` が偽の間は 10Hz 再駆動を止める (RC-2 と同じ処方)、
(c) 最低限、出荷前に「コレクション root を開いた状態」ではなく
「待機モーダルを出した状態」でも `tray-residency` を 1 回測る。

---

### S1-3 (P2) `CollectionRuntimePhase::Starting` の待ちに上限・backoff・終端昇格が無い

**根拠**
- `poll_collection_action_catalog` (`src/ui_dialogs/collections.rs:1786-1789`):
  `CollectionRuntimePhase::Starting => return Ok(None)`。
  呼び出し側 (`saved_group_actions.rs:513`) は `Ok(None)` を 100ms 再駆動に変える。
- `Starting` から抜ける経路は `collections.rs:2152` (`Ready`) と `:2172` (`Closed`) の 2 つだけで、
  どちらも actor の応答を待つ。**deadline も試行上限も無い。**
- 比較: `Inert` / `Failed` / `Closed` は `collections.rs:1790-1792` で即 `Err` になり終端する。
  `Failed` スロットも `:1774-1785` で 1 回だけ再試行して終端する (この設計自体は良い)。

**想定される失敗 (推定)**: actor の起動が完了しない環境で、モーダルが 10Hz で回り続ける。
「中止」はあるので恒久ロックではないが、再レビュー RC-2 (`Busy` / `Starting` の 20Hz 無期限)
と同型の待ちを 1 つ増やしている。

**修正方向**: RC-2 と同じ処方を共有する (backoff + 上限超過時の終端昇格 →
「コレクションを利用できません。」へ落として要求を退役)。

---

### S1-4 (P2) 「追加先の本を開く」がメニューとキーで別経路になり、キーのときだけモーダルが出る

**根拠**
- メニュー `MenuCommandId::BooksOpenActiveBook` (`src/ui_main.rs:6036-6040`):
  `fav_nav = Some(self.active_book_folder_path());` — 通常の非同期フォルダナビ。モーダル無し。
- キー `KeyAction::GridOpenActiveBook` (`src/app/saved_group_actions.rs:356-357`):
  対象パスは即決まるのに `start_saved_book_scan` → `ScanningBook` →
  `saved_group_open_modal_visible()` が真 → 「本を開く準備中...」のモーダルで背面を遮断し、
  走査完了後に `load_folder_with_scan_owned(.., OpenRequestOwner::Navigation)`。
- そのうえ `MENU_COMMAND_SPECS` では両者が同じ action に紐付いた
  (`src/keymap.rs:2882-2894` の `action: Some(KeyAction::GridOpenActiveBook)`) ので、
  **メニューにキー名が表示されるのに、押すと挙動が違う。**

**想定される失敗 (推定)**: 同じ「追加先の本を開く」を、メニューからは今までどおり
(背面操作可・進捗は通常表示) 、キーからはモーダルで全操作が止まる形で体験する。
ページ数の多い本ほど差が大きい。

**注記**: 一覧を待つ必要がある番号 / 前後 / コレクション系でモーダルを出すのは利用者が
承認した設計 (§23.12) だが、`GridOpenActiveBook` は**待つ一覧が無い**ので、この 1 つだけ
モーダルに入る理由が薄い。

**修正方向**: `GridOpenActiveBook` はメニューと同じ通常ナビ経路へ寄せる
(= `SavedGroupOpenTransition` を作らず `AddressBarNav::Direct` 相当を返す)。
逆にモーダルを正とするなら、メニュー側も同じ経路に寄せて 1 つの owner にする。

---

### S1-5 (P2) ツールバーの「本の管理…」が、従来消していなかった `book_list_cache` を消すようになった

**根拠**
- `src/ui_main.rs:9981-9984` (差分): 旧 `self.show_book_manager = true;` の 1 行が
  `self.open_book_manager_from_action();` に置き換わった。
- `open_book_manager_from_action` (`src/app/saved_group_actions.rs:116-121`) は
  `book_manager_rename_name` の更新に加えて `self.book_list_cache = None;` と
  `request_book_list_refresh()` を行う。
- `book_list_cache` はツールバーの固定本ボタンの実体でもある
  (`src/ui_main.rs:8346-8350`)。`src/books.rs:665-669` のコメントは
  「一覧を `None` にして再走査すると、その間だけ固定本棚ボタンが全て消えてツールバーが
  縮み、再取得時に元へ戻るちらつきになる」と、まさにこの退行を警告している。
- さらに `request_book_list_refresh` は `book_op_pending.is_some()` で早期 return する
  (`src/app.rs:35297-35299`)。**別の本操作が進行中だとキャッシュだけ消えて再取得が始まらない。**

**想定される失敗 (推定)**: 本の追加 (append) 直後にツールバーの「本の管理…」を開くと、
固定本ボタンが消えたまま、その操作が終わるまで戻らない。

**注記**: メニュー側 (`src/ui_main.rs:6059-6063`) は元から `book_list_cache = None` を
していたので、そちらは挙動不変。`feedback_only_unify_what_actually_diverges` の観点で、
**実際には分岐していなかった箇所まで統一して挙動を変えている**。

**修正方向**: `open_book_manager_from_action` から `book_list_cache = None` を外し、
管理画面側の既存の遅延取得 (`src/ui_main.rs:6070`, `:6987`, `:7458` の
`if book_list_cache.is_none() && book_op_pending.is_none() { … }`) に任せる。
無効化が必要なら `request_book_list_refresh()` が実際に始まったときだけ消す。

---

### S1-6 (P2) 待機モーダルが Esc でもバックドロップクリックでも閉じない

**根拠**
- `render_saved_group_open_modal` (`src/app/saved_group_actions.rs:276-288`) は
  `egui::Modal::new(..).show(..)` の戻り値 (`should_close()`) を捨てており、
  `cancel` は「中止」ボタンのクリックだけ。`dialog_escape_pressed` も使っていない。
- 既存の `smart_folder_transition` モーダル (`src/app/smart_folder.rs:7880-7957`) も
  同じ形なので parity は取れている。

**想定される失敗 (推定)**: キー操作で開いた待機をキーで取り消せない。マウスで
「中止」を押すしかない。キーボード主体の操作として始めた導線なので違和感が残る。

**修正方向**: `dialog_escape_pressed(ctx)` (IME 変換中は false) を
`cancel_saved_group_open_request(id)` に繋ぐ。TextEdit を含まないので IME helper の
追加要件は無いが、CLAUDE.md の規約どおり直接 `key_pressed(Escape)` は使わない。

---

### S1-7 (P2) 待機区間に perf 計装が無く、「1回の操作で開く」の待ち時間を測れない

**根拠**
- `saved_group_actions.rs` 全体に `crate::perf::` の呼び出しが 0 件。
  記録されるのは `bump_input_seq("grid-book-key" / "grid-collection-key", ..)`
  (`:561, 596`) と、再ターゲット時の `crate::logger::log` (`:591-594`) だけ。
- 比較: コレクションの他の非同期区間は `queued_at = crate::perf::is_enabled().then(Instant::now)`
  で計装済み (`src/app/collection_grid.rs:1189`)。

**想定される失敗 (推定)**: 「キーを押してから開くまでが遅い」という報告が来ても、
本棚走査・カタログ待ち・採用のどこで時間を使ったかを perf ログから切り分けられない。
再レビュー RE-3 (起動時バックアップの未計装) と同型。

**修正方向**: 要求開始 / 各 phase 遷移 / 採用に `perf::event` を 1 つずつ。
`is_enabled()` ガード付き、path とコレクション名は載せない (B-6 の合意どおり)。

---

### S1-8 (P3) リングの `AddToCollection` が、グリッド文脈 + フルスクリーン中に無言の no-op になる

`src/app/gamepad_input.rs:6389-6407`。`RingShortcutContext::Grid if self.fullscreen_idx.is_none()`
に一致しないと最後の `RingShortcutContext::Grid => {}` に落ちて何も起きず、通知も出ない。
`apply_ring_add_to_book` (`:6373-6387`) の Grid arm には条件が無いので形が揃っていない。
安全側ではあるが、`feedback_no_untestable_branches` の観点では理由付きの通知が要る。

### S1-9 (P3) `adopt_saved_group_ready` の退役経路だけ `detach_saved_group_book_list` を呼ばない

`src/app/saved_group_actions.rs:554-558`。`poll_saved_group_open` の同型経路 (`:440-445`) と
`cancel_saved_group_open_request` (`:204-215`) は detach を呼ぶ。現状 adopt に到達するのは
`ReadyBook` / `ReadyCollection` の 2 phase だけなので `book_op_pending` に自分の id が
残っている可能性は低いが、3 つある退役入口のうち 1 つだけ形が違う。

### S1-10 (P3) 新規 49 action に、`EXTERNAL_TOOL_ACTIONS` 相当の表テストが無い

`src/keymap.rs:11568-11578` は `EXTERNAL_TOOL_ACTIONS` について
`ALL_ACTIONS` 収録 / `context()` / `trigger()` / `default_chords().is_empty()` /
`is_user_facing()` / ini round-trip をまとめて固定している。
`SAVED_GROUP_ACTIONS` / `BOOK_LOCATION_ACTIONS` / `COLLECTION_LOCATION_ACTIONS` には同等が無く、
`collection_add_actions_have_empty_defaults_and_distinct_contexts` (`:13877-`) が
追加 3 action だけを見ている。

**ただし実害は小さい**: `all_actions_have_unique_names_and_parse_back` (`:10201-10219`) が
全 action の ini 名の一意性と round-trip を、`the_checked_in_default_keymap_matches_what_the_app_writes`
(`:13831-13846`) が `docs/keymap.ini.default` との 1 対 1 を、ALL_ACTIONS の
ソース突き合わせテスト (`:9872-9877`) が収録漏れを、それぞれ機械的に塞いでいる。
足りないのは `book_slot_number()` / `collection_slot_number()` の 1↔20 対応の固定。

### S1-11 (P3) 別窓から番号キーを押すと、原因の分からない文言が出る

`src/app/saved_group_actions.rs:322-325`。`smart_folder_source_lease()` は
`projected_viewer_context_id() != viewer_context_main()` のとき `None` を返す
(`src/app/smart_folder.rs:1496-1500`) ので、メイン以外が投影されている状態では
「メインの表示状態を確認できません」というトーストになる。安全側の拒否だが、
利用者には何を直せばよいか分からない。

### S1-12 (P3) モーダルのスピナーが 10Hz でしか回らない

`saved_group_actions.rs:493, 513` の `request_repaint_after(100ms)`。
`ui.spinner()` は連続再描画を前提にしているので、待機表示がカクつく (推定)。

### S1-13 (P3) `ScanningBook` / `WaitingBookList` に進捗表示が無い

`saved_group_actions.rs:267-274` のラベルは固定文字列 3 種のみ。比較対象の
smart folder 待機モーダル (`src/app/smart_folder.rs:7895-7911`) は件数・確認済みフォルダ数・
現在のフォルダ名を出している。ページ数の多い本では「止まったのか」が分からない。

### S1-14 (P3) 本作業由来の送り項目がバックログに 1 件も無い

`docs/next-release-backlog.md` の未コミット差分は §1.260 (敷き詰め表示) と §1.261
(編集反映のクリップボードコピー) の利用者要望 2 件だけで、§23.12 由来の送り
(番号の並べ替え UI、番号列、S1-4 の経路統一など) は 1 件も起票されていない。

### S1-15 (P3) `collection_action_catalog()` が `Failed` スロットを「更新中です。」と説明する

`src/ui_dialogs/collections.rs:1742-1749`。`catalog_request` が `Failed` のとき
`!matches!(.., Idle)` が真になるので Err は「コレクション一覧を更新中です。」になる。
実際には更新中ではなく直前の読み取りが失敗している。この Err はモーダル経路では
すぐ `poll_collection_action_catalog` の `Failed` 分岐に置き換わるので利用者には出ないが、
述語の意味と文言がずれている (`feedback_one_owner_per_spelling`)。

---

## 2. 確認して問題なしだったもの

**keymap 規約 (項目 1)**
- 追加 49 action すべてが `ALL_ACTIONS` に入っている。収録漏れは
  `src/keymap.rs:9866-9880` がソースの enum 定義を突き合わせて機械的に検出する。
- `ini_name()` の一意性と `from_ini_name` round-trip は
  `all_actions_have_unique_names_and_parse_back` (`:10201-10219`) が全 action で固定。
- `docs/keymap.ini.default` との 1 対 1 は `the_checked_in_default_keymap_matches_what_the_app_writes`
  (`:13831-13846`) が生成物比較で固定。差分の 52 行は全て `= none` で、説明文も
  `description()` と一致している。
- `context()` は 46 が `KeyContext::Grid`、追加 3 つが `Grid` / `FsImage` / `FsVideo`
  (`:5103-5155`, `:5299`, `:5357`, `:5507`)。`trigger()` は全て `Press` (`:5578-`)。
  `default_chords()` は全て空 (`:6102-`, `:6299`, `:6372`, `:6528`)。既存の既定キーを奪わない。
- 操作カスタマイズ UI は `command_catalog()` → `KeyAction::all()` (`:2497-2502`) なので
  自動で載り、`is_user_facing()` (`:3574-3582`) の除外対象にも入っていない。
- メニュー連携は `MENU_COMMAND_SPECS` の 4 箇所が `action: None` から実 action へ変わり
  (`:2828-2948`)、`assert_eq!(spec.scope, spec.action.context())` 等の既存テスト
  (`:10446-10460`, `:10500-10504`) を通る形になっている。
- `install_global_native_video_shortcuts` (`:8122-8129`) は `KeyContext::FsVideo` を
  拾うので、`VideoAddToCollectionTarget` に割り当てたキーは native 動画へも届く。
- お気に入り `GridOpenFavorite1..20` と比べて欠けている型は無い
  (slot テーブル + `*_slot_number()` + `ini_name` / `description` の slot 分岐 + context/trigger/default)。

**番号と前後の意味 (項目 2)** — S1-1 以外
- 本の番号は `list_books` (`src/books.rs:656-661`) の小文字名順 = 製本の管理の表示順。
  コレクションは `catalog_position` 順 = 管理画面の表示順。どちらもマニュアルの記述と一致。
- 前後の循環は `ordered_target_index` (`src/app/saved_group_actions.rs:12-22`) が
  `(i+1)%len` / `(i+len-1)%len` / 現在位置不明なら次=先頭・前=末尾。
  **既存のお気に入り循環 `apply_favorite_cycle_nav` (`src/app/gamepad_input.rs:5574-5579`) と
  式が一字一句同じ**で、parity が取れている。
- 21 件目以降と存在しない番号は `ids.get(slot-1)` / `rows.get(slot-1)` が None を返し、
  「コレクション N は未登録です」/「本 N は未登録です」を出して要求を作らない
  (`saved_group_actions.rs:67-72, 98-103`)。0 件のときの前後も
  「本が登録されていません」/「コレクションが登録されていません」で終端する。
- `is_snapshot_active()` のガード (`:310-313`) はお気に入り循環の同じガード
  (`gamepad_input.rs:5562-5567`) と揃っている。

**モーダルと待機 (項目 3)**
- UI スレッドでブロッキング待ちをしていない。本は worker thread + `mpsc` + `try_recv`
  (`:171-193, 474-494`)、カタログは既存の `CollectionReadSlot` を poll するだけ
  (`collections.rs:1770-1826`)。`recv()` / `recv_timeout()` は 1 箇所も無い。
- **終了経路を全て辿った**: `WaitingBookList` は「自分の List が進行中」以外は必ず
  解決か toast 終端。`ScanningBook` の `Disconnected` は
  「本の読み込みが中断されました」で終端 (`:489-492`)。
  カタログの `Inert` / `Failed` / `Closed` は即 Err (`collections.rs:1790-1792`)、
  `Failed` スロットは 1 回だけ再試行して 2 回目に Err (`:1774-1785`)、
  0 件と範囲外は上記のとおり。**恒久ループになる経路は `Starting` (S1-3) だけ。**
- `common_modal_dialog_open` への登録は `modal_dialog_block_reason` 経由
  (`src/app.rs:18765`)。`ui_main.rs` の handler-level テストが
  `assert_eq!(app.modal_dialog_block_reason(), Some("saved_group_open"))` と
  モーダル中のセル click が全画面を開かないことを固定している。
- グリッドの追加キーは `!any_dialog_open()` を含む既存ガードの中
  (`src/app.rs:74691-74707`) で、`GridAddToActiveBook` と同じブロックに置かれている。
- `handle_keyboard` は `shortcuts_blocked_by_text_input` → `KeyboardOwner::Modal` で
  早期 return するので、モーダル中に同じキーを連打しても二重要求にならない
  (加えて `saved_group_open.is_some()` の明示ガードが `:318-321` にある)。
- IME: モーダルに TextEdit が無いので `ime_focus` helper の追加要件は発生しない。
  管理画面 (`collections.rs:3393-3397`) は既存どおり `crate::ime_focus::add_singleline`。

**所有境界と stale (項目 4)**
- 要求は `SmartFolderSourceLease` (context_id + surface_generation) と
  単調増加の要求 ID を持つ (`:44-52, 146-149`)。
- 退役は 3 入口: `poll_saved_group_open` の先頭 (`:440-445`)、
  `retire_saved_group_open_if_replaced` (`:225-236`)、
  ナビ裁定前の一括 cancel (`src/app.rs:74973-74989`)。
  `saved_group_other_main_nav_pending` (`:123-144`) は folder_nav / folder_pane /
  smart folder / bookmark / startup / archive convert / remote 制御を網羅する。
- **採用は「通常のナビゲーション裁定で勝った場合だけ」になっている**:
  `saved_group_ready_nav()` は候補 (`AddressBarNav::SavedGroupReady(id)`) を返すだけで、
  `input_nav` の最後尾 `.or_else(..)` に置かれ (`src/app.rs:74991-74996`)、
  他候補があれば先に cancel される。`adopt_saved_group_ready` は id 一致・
  sidecar 非活性・lease 一致・他ナビ非進行を再確認してから実行する (`:542-558`)。
- 履歴は `open_collection_grid_from_navigation` (`src/app/collection_grid.rs:1139-1148`) の
  `record_collection_nav_transition` を通るので、明示 Open と同じ 1 本。
  テストが「未採用の間は `folder_nav_back_stack.len()` が増えない」を 6 箇所で固定している。
- **sidecar 復元との引き継ぎ**: `poll` / `modal_visible` / `ready_nav` / `adopt` /
  `retire` の 5 つすべてが `sidecar_restore_active()` を先に見る
  (`:239, 314, 435, 527, 550`)。復元中はモーダルを描かず要求を保持し、終端後に
  lease を再確認する。要求を「進行中の別ナビ」として誤検出しないよう
  `retire_saved_group_open_if_replaced` は復元中だけ早期 return する。
  2 本のテスト (`ready_book_waits_for_sidecar_modal_then_rechecks_source_before_adoption`、
  `sidecar_terminal_drops_book_request_if_source_changed_during_restore`) が両方向を固定。
- コレクションの「読み込み中の root が可視採用された時点で引き継ぐ」は
  `adopt_saved_group_ready` が最新カタログで再解決し、ずれたら
  `saved collection open retargeted after catalog update` をログに残す (`:590-595`)。
  二重 open / 二重 push は、候補が 1 本で `take()` 後に phase を差し替える形なので発生しない。

**追加キー 3 面 (項目 5)**
- グリッドは `grid_selection_indices()` (`src/ui_fullscreen.rs:43033-43042`) で
  checked 優先 → selected。本棚の `add_grid_selection_to_active_book` と同じ helper。
- 静止画 FS は `GridItem::Image` だけを受け、それ以外は
  `file_operation_refusal().message("コレクションに追加")` で理由付き拒否
  (`src/ui_dialogs/collections.rs:2001-2011`)。**親コンテナへ丸めていない。**
  テストが PDF ページで「PDF 内のページ」を含む文言を固定している。
- 動画・音声は `GridItem::Video | GridItem::Audio` のみ (`:2023-2031`)。
- 追加先 UUID と実パスは入力時に捕捉し (`add_captured_paths_to_collection`、`:2046-2059`)、
  その後の選択変更・追加先変更・actor 応答を読み直さない。
  `collection_shortcut_freezes_grid_target_and_checked_paths_before_actor_reply` が固定。
- 非リピート: 4 経路すべて `consume_action_no_repeat` / `!key.repeat`
  (`collections.rs:1983`, `ui_fullscreen.rs:27325, 27962, 45204`, `native_video.rs:12215`)。
- **音声 VST プラグイン画面の隔離は成立している**: `dispatch_native_video_key_event` の先頭
  (`src/app/native_video.rs:11623-11636`) が `music_vst_shell` のとき Escape 以外を
  `Blocked(MusicVstShell)` で返すので、追加した arm (`:12215-12222`) には到達しない。
  マニュアルの「音声の VST プラグイン画面を操作中は受け付けません」と一致する。
- **別窓で main の selection を読まない**: `src/app/tests.rs:1412-1523` の
  `collection_shortcut_from_mounted_detached_keeps_main_selection_and_scroll` が、
  detached bundle 上で実行した追加が detached のパスを捕捉し、main の
  `items` / `selected` / `checked` / `scroll_offset_y` が不変であることを固定している。
- **detached 凍結ルール**: 差分は detached 述語 / viewport / host identity / focus /
  geometry / window lifecycle を一切触っていない (`git diff` に該当箇所なし)。
  `docs/detached-rework-plan.md` §11 に 2026-09-20 付で記録があり、
  「既存 context owner へ入力入口を接続する機能で §2 が禁じる症状パッチではない」と
  構造合意が明記されている。**規約どおりの手続きが踏まれている。**
- 追加先未設定は `collection_shortcut_target` (`:2035-2044`)、編集不可は `can_edit()`、
  進行中は `operation.is_idle()`、上限 10,000 件と重複は既存の actor transaction
  (`ToolbarAddSnapshot` 経路) がそのまま通知を出す。

**リング / ゲームパッド (項目 6)**
- `RingActionId::AddToCollection` は `as_str` / `from_str` の round-trip、
  `is_valid_for_context` の 3 文脈、`available_for_context` /
  `available_for_mouse_button_context` の 3 リストすべてに入っている
  (`src/ring_shortcut.rs:539, 941, 1091, 1203, 1360, 1395, 1433, 1498, 1532, 1569`)。
  `collection_add_round_trips_in_all_ring_contexts` (`:3185-3202`) が 3 文脈を固定。
- `docs/ring-keyaction-parity.md` は 125→126 / 123→124 / 111→112 と、
  「✅ 対応済み」への `AddToCollection`→`Grid/Fs/VideoAddToCollectionTarget` 追記が入っている。
- `ring_actions_are_classified_for_key_action_parity` (`src/keymap.rs:10097-10099`) の
  `key_handled` に `"AddToCollection"` が追加され、未分類 0 が維持されている。
- ラベルは文脈共通の「追加先のコレクションに追加」。動画でも**フレームでなくファイル**を
  登録するので、`AddToBook` のような文脈別ラベルは不要で正しい。

**管理画面を開く 3 操作 (項目 7)**
- 3 つとも冪等な「開く / 前面化」で、トグルではない
  (`show_favorites_editor = true` / `open_book_manager_from_action` /
  `open_collection_manager(None)`)。既に開いていても二重には開かない。
- `handle_keyboard` は `KeyboardOwner::Modal` で早期 return するので、
  他のモーダル表示中 (sidecar 復元を含む) にキーが消費されることはない。
- フルスクリーン中はこれらが `CommandScope::Grid` の keyboard 経路にしか無いので発火しない。
- `configured_book_and_manage_keys_reach_the_grid_dispatcher`
  (`saved_group_actions.rs:855-907`) が、物理リピートで再度開かないことまで固定している。
- `MenuCommandId::CollectionsManage` のメニュー側 (`src/ui_main.rs:6283`) も
  `open_collection_manager(None)` で、キーと同一。

**毎フレームコスト / 規約 (項目 9)**
- `handle_keyboard` の `SAVED_GROUP_ACTIONS.iter().find(..)` は 49 回
  `consume_action_no_repeat` を呼ぶが、未割り当て action は
  `overrides.get()` が None → `default_chords()` が空 → ループ 0 回で false を返すため
  `ctx.input_mut` のロックを取らない (`src/keymap.rs:7731-7762`)。
  既定状態では HashMap 参照 49 回のみで、`feedback_shape_is_not_cost` が問題にした
  線形スキャンには当たらない。
- 毎フレームの clone / 全件走査は追加されていない。`collection_action_catalog()` は
  `Vec<CollectionId>` を作るが、**キー押下時と poll 時にしか呼ばれない**
  (毎フレーム経路には無い)。
- `perf::event` の常時コストは追加ゼロ (S1-7 は逆に不足の指摘)。
- `python scripts/check_ui_glyphs.py` = `ok: no dangerous glyphs found` (exit 0)。
- UI 露出文言に actor / revision / snapshot / UUID / catalog などの内部語は無い。
  ログ (`:591-594`) の英語と `{:?}` は利用者に出ない経路。
- マニュアル追記 (`shortcuts.html` / `tut-books.html` / `tut-collections.html` /
  `tut-favorites.html`) にバージョン表記・内部用語は無く、記述方針に沿っている。

**`BookOpIntent` の変更 (副作用確認)**
`Unrelated` → `List { navigation }` の置き換えは `request_book_list_refresh`
(`src/app.rs:35311`) の 1 箇所のみ。`blocks_rename_migration` は
`!matches!(self, Unrelated)` から `matches!(self, SourceMutation | Delete{..})` へ変わったが、
既存 3 intent の結果は不変 (`src/books.rs:221-225`)。呼び出し元は `src/app.rs:32561-32570` の
3 箇所だけで、`List` は旧 `Unrelated` と同じく非ブロック。**回帰は無いと判断した。**

---

## 3. マニュアル記述 × 実装の行単位照合 (RE-5)

| 記述 | 実装 | 判定 |
| --- | --- | --- |
| `shortcuts.html` 「お気に入り・本・コレクションは、前/次、1〜20 番で開く、管理画面を開く操作を…割り当てられます」 | 3 系統とも揃っている (お気に入りは `GridFavoritePrev/Next` + `GridOpenFavorite1..20` + 新規 `GridManageFavorites`) | 一致 |
| 同「本とコレクションには『追加先を開く』操作もあります」 | `GridOpenActiveBook` / `GridOpenCollectionTarget` | 一致 |
| 同「番号は固定ボタンだけでなく管理一覧の全件に対応し、本は名前順、コレクションは管理順です」 | `list_books` の名前順 / `ORDER BY catalog_position` | 一致 |
| 同「いずれも標準キーはありません」 | `default_chords()` = `ChordList::EMPTY`、`keymap.ini.default` も `= none` | 一致 |
| `tut-books.html` 「前/次は端で循環します」 | `ordered_target_index` の剰余 | 一致 |
| 同「本棚一覧の読み込みが必要なときも、キーを1回押せば読み込みを待って開きます。待機画面の『中止』で取り消せます」 | `WaitingBookList` + `book_op_pending` への相乗り + 「中止」ボタン | 一致 |
| `tut-collections.html` 「一覧の読み込み・更新中に押した場合も、1回の操作で最新の管理順を待って開きます」 | `WaitingCollectionCatalog` + `catalog_revision >= wanted` の要求 | 一致 |
| 同「同じ『追加先のコレクションに追加』は…リングショートカットやマウスジェスチャにも割り当てられます。表示している実ファイルを登録し、追加時に画面は移動しません」 | 3 文脈のリング + `ring_add_to_collection_uses_real_sources_without_switching_grid` | 一致 |
| `collections.html` (コミット済) 「音声の VST プラグイン画面を操作中は、このキーを受け付けません」 | `native_video.rs:11623-11636` の `Blocked(MusicVstShell)` | 一致 |
| `tut-favorites.html` 「前/次のお気に入りへ移動する操作と『編集』を開く操作にもキーを割り当てられます」 | `GridFavoritePrev/Next` + `GridManageFavorites` | 一致 |
| — | 番号が**作成順で固定・並べ替え不可・削除で繰り上がる** | **未記載 (S1-1)** |

---

## 4. テストが固定しているもの / 欠けている回帰 (項目 10)

`saved_group_actions.rs` の 10 テストが固定している内容:

1. `cycle_wraps_and_unknown_current_uses_the_directional_edge` — 循環と端の起点。
2. `numbered_book_open_attaches_to_cold_list_and_adopts_once_from_full_order` —
   冷たい一覧への相乗り、未採用中は generation / 履歴が不変、採用は 1 回。
3. `ready_book_waits_for_sidecar_modal_then_rechecks_source_before_adoption` —
   sidecar 復元中は不可視・不採用、終端後に採用。
4. `sidecar_terminal_drops_book_request_if_source_changed_during_restore` —
   復元中に表示元が変わったら終端時に破棄。
5. `ready_book_yields_candidate_to_later_pending_main_navigation` — 後発ナビに譲る。
6. `cancelled_cold_book_request_only_warms_cache_and_late_nav_cannot_open` —
   中止後の遅着で開かない。
7. `synthetic_old_book_marker_is_not_cycle_current_and_late_pane_nav_retires_owner` —
   Folder 以外の surface は現在位置にしない。
8. `configured_book_and_manage_keys_reach_the_grid_dispatcher` —
   keymap 設定からの到達と物理リピートの非再実行。
9. `later_smart_root_request_retires_unadopted_book_open` — smart folder に譲る。
10. `remote_control_acquisition_retires_unadopted_book_open` — remote 取得で退役。

加えて `ui_dialogs/collections.rs` に 6 本、`ui_main.rs` に 1 本
(`saved_group_modal_blocks_real_image_cell_loading_and_ready_until_exact_cancel`)、
`app/tests.rs` に 1 本 (別窓)、`native_video.rs` に 1 本、`ring_shortcut.rs` に 1 本。

**欠けている回帰 (優先順)**
- **モーダルの終了経路の網羅**: `Starting` のまま終わらないこと (S1-3)、
  `ScanningBook` の `Disconnected`、本一覧の取得失敗、コレクション 0 件、
  番号が範囲外。いずれも handler-level で書けるが、現状は 0 件以外テストが無い。
- **連打**: 同じ番号キーの 2 回押し (2 回目が toast で弾かれる)、
  別番号への押し直し (旧要求が退役して新要求だけが残る)。現状どちらも無い。
- **`GridOpenCollectionTarget` の追加先が待機中に削除された場合** —
  `collection_action_target` の `ids.contains` フィルタを通って Err になる経路。
- **`book_slot_number()` / `collection_slot_number()` の 1↔20 対応**と、
  49 action の context / trigger / 空既定をまとめて固定する表テスト (S1-10)。
- **別窓での番号キー** — `smart_folder_source_lease()` が None を返す経路の固定。
- **`adopt_saved_group_ready` の再ターゲット** — Ready 後にカタログが変わり、
  別の ID を開いてログが残ることの固定 (現在は `logger::log` のみで検証なし)。
