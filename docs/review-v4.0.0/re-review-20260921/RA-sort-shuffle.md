# 再レビュー RA: ソート UI 統一 (案 C) / シャッフル順 / 関連 UI

対象: `540fdc216` / `c12c84562` / `53752314e`、および `6f720e42c`。
基準: コミット済み HEAD `a5a1fc43b`。**アプリは起動しておらず、cargo も実行していない。実行時の観測は無い。**
以下の「〜になる」はすべてコード読解に基づく推定で、根拠は HEAD 時点の `file:line`。
作業ツリーの未コミット差分 (KeyAction 関連 / `src/app.rs` `src/ui_fullscreen.rs` `src/ui_dialogs/collections.rs`
`src/app/native_video.rs` `src/app/tests.rs` `src/keymap.rs` と docs) には触れていない。それらのファイルは
`git show HEAD:<path>` で読み、行番号も HEAD のものを書いた。

`6f720e42c` の `src/ui_main.rs` / `src/app.rs` 差分を確認したが、ソート周辺の識別子
(`current_grid_order` / `current_reader_order` / `details_header*` / `page_order_locked*` / `sort_lock` /
`details_order`) は 1 件も含まれず、スマートフォルダ移行と staged 読み込みだけだった。本件とは独立。

---

## 0. 元指摘ごとの判定

| ID | 判定 | 根拠 (HEAD) |
| --- | --- | --- |
| A-1 ツールバー / 表示メニューのソートが効かず global を書き換える | **部分的** | UI 3 経路は解消: `src/ui_main.rs:6582-6594`(表示メニュー) / `:8965-8990`(Buttons) / `:9085-9106`(Dropdown) は Collection 分岐で `settings.sort_order` を書かず `SetOrder` だけ送る。`src/app.rs:20885-20887` で `apply_sort_change_reload` が早期 return。**残存**: gamepad リングピッカー経路 → RA-1 |
| A-2 列ヘッダソートが手動順を上書きする | **解消** | `src/app.rs:52622-52638`(`details_header_sort_active` / `details_header_sort_locked`)、`src/ui_main.rs:15797-15799`(`sort_enabled`)、`src/app/collection_grid.rs:1054-1057`(open 時 reset)、履歴復帰 `src/app.rs:19528` `:23741` `:75016` `:75241` も `open_collection_grid` 経由。テスト `collection_grid.rs:2373` |
| A-5 ウィンドウタイトル | **解消** | `src/app.rs:9960-9961` + 呼び出し `:72752-72767`。Root 限定 filter あり。prepared 未着時は `コレクション: 読み込み中 - mimageviewer` |
| A-9 「並び順」の無効理由 / ✓ 表記 | **解消 (ただし理由文が 2 系統)** | `src/ui_main.rs:6244-6248`(disabled hover)、`:6196` `:6211` `:6228`(✓ 表記へ統一)。理由文の二重導出 → RA-5。表示メニュー側に新しい表記不統一 → RA-9 |
| D-1 案 C (手動順を一級市民に) | **解消** | 単一 typed 述語 `collection_grid_root_order()` (`src/app/collection_grid.rs:415-448`) を `grid_sort_lock_reason` / `details_header_sort_active` / `apply_sort_change_reload` / `main_window_title` が共有。新しい `items_are_collection_view` bool は追加されていない |
| §1.244 ZIP/PDF/画像本の列ヘッダ | **解消 (opt-in 利用者に行動変化)** | `src/app.rs:52626` が `page_order_locked_for_current_view()` を `details_header_sort_active` に通す。`rebuild_details_order` (`:52816`) も同じ述語を見るので details 並びも固定される。副作用 → RA-12 |
| シャッフル実装 (方式 C) | **解消** | 単一 helper `effective_collection_order` (`src/collection_store/model.rs:389`、Shuffle は `:397-416`)。唯一の呼び出しは `src/collection_store/prepare.rs:526` で、Grid / navigation / Remote / export が全部そこを通る。digest = seed LE 8 bytes ‖ UUID raw 16 bytes、tie-break は UUID bytes 昇順 (`model.rs:399-413`) |
| DB v1→v2 移行 | **解消** | 単一 transaction (`src/collection_store/db.rs:677-685`)、移行前バックアップ失敗で中止 (`:90-112`)、`schema_version` 二重 gate + TOCTOU 検出 (`:53-65` `:75-83`)、将来版は `IncompatibleSchema` で拒否 (`:64` `:76-78`)、空 DB 置換なし。テスト `collection_store/tests.rs:395` `:616` `:644` `:1632` |
| Remote (IPC 版 / Web 表示) | **解消** | 版定数は 1 箇所 `crates/remote-ipc/src/lib.rs:31`。**HEAD では 58**(`8dbad53e5` がさらに 57→58)。56/57 の取り残しなし。exact-equality ハンドシェイクで v56 exe は最初のフレームで拒否 (`src/remote_ipc/pipe.rs:2542-2566`、client 側 `crates/remote-web/src/ipc_client.rs:1820-1832`)。wire enum は名前タグ (`lib.rs:1343-1353`)、core→wire は網羅 match (`src/remote_ipc/persistent_collections.rs:2023-2036`)。Web は「シャッフル」を表示 (`crates/remote-web/web/app.js:4895-4901`)。Web の default 分岐 → RA-15。文書のズレ → RA-16 |
| 「反映待ち」(fullscreen leaf 保留) | **解消** | `src/app/collection_grid.rs:1308-1329`(`collection_grid_refresh_waits_for_viewer`)、`src/app.rs:52660-52662` で Deleted/Failed を優先。fullscreen close で解消することをテストが固定 (`collection_grid.rs:3940-4025`)。永久残留の経路は見つからなかった |
| Auto サムネ比率 cache (`53752314e`) | **部分的** | UI スレッドの同期 DB / blocking `recv` は無い。100 ms 待ちは `collection-grid-prepare` worker 上 (`src/app/collection_grid.rs:257-263`、spawn は `:1238-1250`)。epoch bump と Get enqueue は同一 lock 内 (`src/auto_aspect_cache.rs:333-341` / `:377-384`)、古い epoch は `adopt_lookup` で棄却 (`:296-299`)。**残存**: 削除時の行掃除が無い → RA-6、毎フレーム O(N) → RA-7 |
| ソート Dropdown の popup style (`c12c84562`) | **解消** | `src/ui_main.rs:1586-1597` は `egui::style::StyleModifier` を `ComboBox::popup_style` へ渡す局所適用で、`ctx.set_visuals` / `set_theme` を呼ばない。global へ漏れないことをテストが固定 (`src/ui_main.rs:23821-23868`、末尾 `assert_eq!(ctx.style().spacing.scroll.floating_allocated_width, 0.0)`) |
| 回帰テスト README §2.2 (a)〜(e) | **部分的** | 下表。UI クリック経路を通る handler-level テストが無い → RA-14 |

### README §2.2 (a)〜(e) 対応表

| 項目 | テスト | file:line | 備考 |
| --- | --- | --- | --- |
| (a) Manual → 「手動順」表示 | `root_order_controls_use_installed_revision_and_preserve_global_sort_and_header_contract` | `src/app/collection_grid.rs:2373` (`:2406-2409`) | `mode == Manual` は検証。ラベル文字列は未検証 |
| (b) Standard(DateDesc) → 日付↓表示 | 同上 | `:2411-2424` | `SetOrder(Standard, FileName)` で代用。`standard_sort` が表示値へ流れることは検証されていない |
| (c) 名前順選択で `SetOrder` が届き `settings.sort_order` 不変 | 同上 | `:2425` `:2443` `:2453` (`assert_eq!(app.settings.sort_order, SortOrder::DateDesc)`) | `start_collection_grid_content_action` を直接呼ぶ。**ツールバー / メニューのクリック分岐は通らない** |
| (d) 物理フォルダへ戻ると global 表示 | 同上 | `:2455-2485` (`collection_grid_root_order().is_none()`) | PhysicalSource 子で検証 |
| (e) 列ヘッダ押下状態から手動順を開くと保たれる | 同上 | `:2383-2390` | `open_collection_grid` 直後に `details_sort_key == Toolbar` |
| 追加: Standard 列ヘッダは行だけ並べ替える | `standard_root_header_sort_only_reorders_rows_not_reader_or_spread` | `:2487` | reader / spread / Home-End を検証 |
| 追加: 「反映待ち」の発生と解消 | `fullscreen_root_order_change_reports_viewer_deferred_until_close` | `:3940` | |

---

## 1. 新規指摘

### RA-1 (P2) gamepad リングピッカーのソート行が、コレクション直下でも global `settings.sort_order` を書き換えて保存する

**根拠**
- `src/app/gamepad_input.rs:211-217` `GRID_PICKER_ROWS` は `RingPickerRowId::GridSortOrder` を無条件に含む。
- `:2991` 現在値は `self.settings.sort_order` から seed する (コレクションの `order_mode` / `standard_sort` を見ない)。
- `:3817-3818` と `:4889-4893` → `:4908-4915` `apply_grid_picker_sort_order` が
  `self.settings.sort_order = sort_order; self.settings.save(); self.apply_sort_change_reload();`。
- `src/app.rs:20885-20887` の早期 return により `apply_sort_change_reload` は何もしない。
- `gamepad_input.rs` 全体を grep しても `grid_sort_lock_reason` / `collection_grid_root_order` /
  `page_order_locked` の参照は 0 件。

**失敗シナリオ (推定)**
コレクション直下でゲームパッドのリングから「ソート」行を回して決定すると、一覧は変わらないのに
全フォルダ共通のソート順が書き換わって保存される。実フォルダへ戻ると並びが変わっている。
これは A-1 の副作用そのもので、ツールバー / メニュー側だけ塞いだ形になっている。
なお本 (`PageOrderFixed`) / 列ヘッダ所有中でも同じくロックを無視するので、**この穴自体は先行不具合**。
ただし A-1 の受け入れ条件が「全経路で global を書き換えない」なので、同型経路として列挙しておく。

**修正方向**
`apply_grid_picker_sort_order` の入口で `grid_sort_lock_reason().is_some()` なら拒否し、
`collection_grid_root_order()` が `Some(Ok(_))` なら `SetOrder` へ振る。ピッカーの表示値 (`:2991`) も
同じ述語から導く。ロック中は行自体を出さない選択もある (`GRID_PICKER_ROWS` を関数化)。

---

### RA-2 (P2) コレクション直下で列ヘッダが無効なとき、hover 理由が「本として表示中や閲覧履歴では…」と出る

**根拠**
- `src/ui_main.rs:15813-15818`:
  ```
  } else if book_sort_locked && sort_key.is_some() {
      response.hover_tip(
          "本として表示中や閲覧履歴では、並び順が固定されます（一覧の並べ替えは使えません）。",
      )
  ```
- `book_sort_locked` は `src/ui_main.rs:15770` で `self.details_header_sort_locked()`、
  その実体は `src/app.rs:52633-52638` で `page_order_locked_for_current_view() ||
  (Collection root かつ Standard でない)`。

**失敗シナリオ (推定)**
手動順 / シャッフルのコレクション直下で列ヘッダにカーソルを載せると、本でも閲覧履歴でもないのに
「本として表示中や閲覧履歴では…」と表示される。利用者から見ると理由が事実と合わない。

**構造的原因**
`GridSortLockReason` という typed な理由が既にあるのに、詳細ヘッダだけ `bool` を受け取って
理由を落としている。案 C が掲げた「同じ 1 つの述語」から表示まで導けていない最後の 1 箇所。

**修正方向**
`details_header_sort_locked() -> bool` を `Option<GridSortLockReason>` (または列ヘッダ専用の typed 理由) に
変え、`hover_tip` を `reason.tooltip()` から出す。`advance_details_best_fit_job` / `details_header_title` /
`draw_details_column_resize_handle` は `is_some()` で従来どおり。

---

### RA-3 (P2) 「表示順 ≠ 読み順」がコレクション直下だけ発生し、通常フォルダと非対称

**根拠**
- `src/app.rs:52547-52556` `current_grid_order()` は Details 表示では `details_order` (列ヘッダ順) を返す。
- `src/app.rs:52561-52581` `current_reader_order()` は **installed な Collection Root のときだけ**
  `visible_indices` (= installed 有効順) を返し、それ以外は `current_grid_order()` にフォールバックする。
- reader 側の consumer: `src/ui_fullscreen.rs:26120`(`get_nav_indices`) / `:26134`(`get_still_image_indices`) /
  `:26780` / `:32701`(slideshow 折り返し) / `:33879` `:33886` `:33974` `:34040` `:34045`、
  `src/app.rs:72053` `:72187` `:72481`(動画/音声の連続再生)、`src/app/native_video.rs:12060` `:12079` `:14081`。

**失敗シナリオ (推定)**
通常フォルダで詳細表示の「サイズ」列を押すと、一覧もフルスクリーンの → も同じ順に従う
(`current_grid_order()` が `details_order` を返すため)。ところが Standard のコレクション直下で
同じことをすると、**一覧は列ヘッダ順なのに → / Home / End / 見開き / スライドショーは installed 有効順**に
従う。利用者から見ると「1 行目を開いて → を押したのに 2 行目ではない項目が出る」。

仕様 (`docs/collection-spec-proposal.md`「シャッフル順の設計」) はこれを明文で決めているので
**実装は仕様どおり**。ただし他のどの一覧とも違う挙動なので、そのままだと不具合報告になる。

**修正方向 (いずれか)**
1. 列ヘッダ所有中は一覧側に「表示順のみ (読み順はコレクションの並び)」を示す (ヘッダ行かアドレス帯に 1 行)。
2. マニュアル `collections.html` に明記する (E 側の作業に 1 項目追加)。
3. 仕様を変えて Collection root でも reader が表示順に従う (= 仕様判断のやり直し)。
   本レビューは 1 か 2 を勧める。

---

### RA-4 (P2) RA-3 の乖離を、音楽ビュー HUD の前/次ファイルボタンだけ取りこぼしている

**根拠**
- `src/ui_music_panels.rs:334-353` `music_navigate_file` の doc comment:
  「表示順 (`current_grid_order`) の隣接する移動可能アイテムへ移動する。キーボード経路
  (`ui_fullscreen.rs` の `FsPageNav` → `adjacent_navigable_idx` → …) をそのまま踏襲する。」
- 実装は `:344` `let display_order = self.current_grid_order().to_vec();`。
- 一方その「キーボード経路」は `current_reader_order()` へ移った
  (`src/ui_fullscreen.rs:26120` `get_nav_indices`、`:26780`)。
- 動画 HUD 側は移行済み (`src/app/native_video.rs:12060` `:12079` `:14081`)。

**失敗シナリオ (推定)**
Standard のコレクション直下で列ヘッダソート中に音声を開くと、音楽 HUD の前/次ファイルボタンと
キーボードの ↑↓ が別の項目へ進む。doc comment が宣言する「同一挙動」が破れている。

**修正方向**
`current_reader_order()` に差し替える。同時に `src/app.rs:39093`(グリッドのキー移動)、
`src/app/gamepad_input.rs:6574`(グリッドの D-pad)、`src/external_tool.rs:2364`、
`src/ui_dialogs/context_menu.rs:1292` `:1309` は**表示順で正しい**ので変えない
(いずれもグリッド上の選択・外部ツール対象で、reader ではない)。

---

### RA-5 (P3) 無効理由が 2 系統から別々に導出され、3 つの UI で文言が食い違う

**根拠**
- 系統 A: `collection_grid_root_order()` が返す `Err(&'static str)`
  (`src/app/collection_grid.rs:423` `:425` `:428` `:431` `:435` `:438` `:441`)。
  上部「コレクション」メニューの disabled hover (`src/ui_main.rs:6244-6248`) と
  「並べ替え…」(`:6254-6274`) がこれを使う。
- 系統 B: `grid_sort_lock_reason()` が返す `GridSortLockReason`
  (`src/app.rs:52645-52683`、文言は `:10244-52` / `:10264-10269`)。
  ツールバーと表示メニューがこれを使う。
- 両者は独立に導出される。具体的な食い違い:
  - fullscreen 保留中 → ツールバー「反映待ち / 表示中の項目を閉じると一覧の更新を再開します。…」
    (`src/app.rs:10247` `:10265-10267`) に対し、コレクションメニューは
    「コレクション一覧の更新を待っています」(`collection_grid.rs:431`)。系統 A に viewer-deferred の
    概念が無い。
  - `installed_items_generation` 不一致 → ツールバー「更新中」(テスト `collection_grid.rs:3998-4006` が
    この分岐を固定している) に対し、コレクションメニューは
    「コレクション直下を開くと使用できます」(`collection_grid.rs:394` 経由)。コレクション直下に**いる**のに
    「開くと使えます」と出る。

**修正方向**
`collection_grid_root_order()` の `Err` を `&'static str` ではなく `GridSortLockReason`
(または両者の共通 typed 理由) にし、文言は 1 箇所 (`GridSortLockReason::tooltip`) が所有する。
RA-2 の修正と同じ方向で、3 つの UI + 詳細ヘッダの 4 箇所を 1 つの述語に寄せられる。

---

### RA-6 (P3) Auto サムネ比率 cache の UUID 行が、コレクション削除時に消えない

**根拠**
- コマンド enum は `Get` / `Upsert` / `Maintenance` / `Shutdown` のみ
  (`src/auto_aspect_cache.rs:243-263`)。DB 側も `get/upsert/count/clear_all/delete_old` だけ
  (`:469-545`)。**単一 UUID を消す経路が無い。**
- 削除経路のどこにも aspect cache の呼び出しが無い:
  `src/collection_store/db.rs:244-258`(`delete_collection`)、
  `src/collection_store/runtime.rs:536-549`(`Command::Delete`)、
  `src/app/collection_grid.rs:1415-1427`(削除通知の反映)。
- 自動 expiry も無い。`ClearAll` / `DeleteOld` はキャッシュ管理ダイアログの 4 ボタンからしか呼ばれない
  (`src/ui_dialogs/cache_manager.rs:121` `:141` `:225`、`src/ui_main.rs:6662`)。

**失敗シナリオ (推定)**
コレクションを作って消すたびに 1 行 (約 60 バイト) が `auto_aspect_cache.db` に残り続ける。
作成したコレクション数に比例する有限の漏れなので実害は小さいが、掃除経路が無いことは
`docs/auto-thumb-aspect-plan.md` §13 に書かれていない。

**修正方向**
`CollectionCacheCommand::Forget { id }` を足し、`Command::Delete` 成功時に actor へ送る
(actor 単独所有の原則は維持できる)。少なくとも仕様へ「掃除は行わない / `DeleteOld` に任せる」と明記する。

---

### RA-7 (P3) `auto_aspect_eligible_total` が collection root で毎フレーム O(items) 走査になった

**根拠**
- `src/app.rs:17851-17874` — Collection root 分岐で `self.items.len()` ではなく
  `self.items` を走査して音声と欠損 placeholder を除外する。
- 呼び出しは `maybe_apply_auto_aspect` (`src/app.rs:18048`) → `poll_thumbnails` (`:37115`) の毎フレーム経路。
  手前の gate は Auto off / `switches_done >= 2` / 入力 500 ms 以内 / cooldown 2 s のみ。

**失敗シナリオ (推定)**
Auto 比率 ON かつ switch 2 回未満の間、コレクション直下を開いていると毎フレーム全件走査が走る。
仕様判断 3 で 1 コレクション 10,000 件を上限に決めたので、上限いっぱいでは毎フレーム 1 万件の
`matches!` になる。割り当ては無いので致命的ではないが、§23.1 で計装を先に入れた方針に対し、
この追加には `perf::event` が無く `--perf-log` で裏が取れない。

**修正方向**
install 時に 1 回数えて session に持つ (件数は install 境界でしか変わらない)。
そのまま残すなら `perf::event` を 1 本入れて `ui.pre_grid_breakdown` と並べて測れるようにする。

---

### RA-8 (P3) ツールバー Buttons の「手動」ボタンの hover が「登録順で表示します」で、事実と違う

**根拠**
- `src/ui_main.rs:8926-8930` — `Shuffle` は「再選択すると新しい順に並べ直します」、`Manual` は
  「登録順で表示します」。
- 手動順は並べ替え画面で任意に並べ替えられる (`manual_position`)。並べ替え後は登録順ではない。
- ラベルも不統一: Buttons は「手動」(`:8919`)、Dropdown の選択肢は「手動」(`:9053`)、
  表示メニューは「手動順」(`:6552`)、上部コレクションメニューは「手動順」(`:6196`)。

**修正方向**
hover を「並べ替え画面で決めた順で表示します」等にする。ラベルは全 4 箇所で
「手動順」(狭い Buttons だけ「手動」) の対応を決め、hover でフルラベルを出す。
なお §2.2-7 の将来案「登録順 (追加日時)」を足すと、この文言と衝突する。

---

### RA-9 (P3) 表示メニュー「ソート順」の中だけ、チェック表記が 2 種類混在する

**根拠**
- `src/ui_main.rs:6552` `:6559` — コレクションの「手動順」「シャッフル（再選択で並べ直す）」は
  `ui.selectable_label(...)`。
- `src/ui_main.rs:6577-6579` — 同じサブメニュー内の通常ソートは `"✓ "` プレフィックス + `ui.button`。
- A-9 は上部コレクションメニューを `selectable_label` → `✓` + `button` に揃えた (`:6196` `:6211` `:6228`) が、
  表示メニュー側に新しい不統一が入った形。

**修正方向**
表示メニューの 2 行も `"✓ "` + `ui.button` に揃える (ツールバー Dropdown は全項目 `selectable_label` で
内部整合しているのでそのままでよい)。

---

### RA-10 (P3) 表示メニューと上部コレクションメニューだけ「すでに選択済み」ガードが無い

**根拠**
- ツールバー Buttons `src/ui_main.rs:8932` / Dropdown `:9058` `:9083` は
  `resp.clicked() && (!selected || mode == Shuffle)` でガードしている。
- 表示メニュー `:6552` `:6559` `:6581` と上部コレクションメニュー `:6195-6197` `:6227-6229` はガード無しで、
  選択済みの項目を押しても `start_collection_grid_content_action` が走る。

**実害**
`src/collection_store/db.rs:270-277` が Shuffle 以外の同値 `set_order` を revision 据え置きで短絡するので
**再 install も thumbnail 再構築も起きない**。無駄な actor 往復 1 回だけ。ただし挙動が経路ごとに違う。

**修正方向**
ガードの有無を 4 経路で揃える (足すか、`set_order` の短絡に任せて全部外すか)。

---

### RA-11 (P3) 列ヘッダ所有中に「コレクションの並びへ戻す」導線が実質無い

**根拠**
- 列ソートの解除は `apply_collection_grid_prepared_install` の
  `src/app/collection_grid.rs:1778-1785` にあり、条件は
  `old.order_mode != prepared.order_mode || old.standard_sort != prepared.standard_sort`。
- `set_order` は同値 Standard を revision 据え置きで短絡する (`src/collection_store/db.rs:270-277`)。

**失敗シナリオ (推定)**
Standard(名前↑) のコレクション直下で「サイズ」列ヘッダを押した状態から、上部コレクションメニュー →
通常ソート → 名前↑ (= 現在値) を選んでも、revision が進まないので列ソートが解除されず、行はサイズ順のまま。
ツールバーは `DetailsHeaderSort` で無効なので、戻す手段は列ヘッダを 2〜3 回押して「ソートなし」に
到達することだけになる。

**修正方向**
上部コレクションメニューでの order 選択時は、reply の revision に関係なく
`reset_details_sort_to_toolbar()` を呼ぶ (§23.2 が書いた「このメニューだけは列ヘッダ所有中も選べ、
成功採用時に列ソートを解除する」を、no-op reply でも満たす)。

---

### RA-12 (P3) 「画像のみフォルダを本扱い」を ON にしている利用者は、通常フォルダでも列ヘッダソートを失う

**根拠**
- `src/app.rs:52626` が `details_header_sort_active()` に `!page_order_locked_for_current_view()` を追加。
- `page_order_locked_for_current_view` → `physical_page_order_locked` (`src/app.rs:10278-10288`) は
  `settings.auto_fullscreen_image_folders_enabled() && items が全て Image` でも true を返す。
- 既定は OFF (`src/settings.rs:6821` `auto_fullscreen_image_folders: false`) なので既定挙動は不変。

**評価**
§1.244 の意図 (本の中でページ順を固定する) と一致しており、ツールバーのソートはもともと
`PageOrderFixed` で無効だったので、失われるのは「列ヘッダだけは使えた」という残りの 1 手段。
**退行ではなく意図した挙動変更**と判断する。ただし opt-in 利用者には見える変化なので、
README 更新履歴かマニュアルに 1 行残すべき。

---

### RA-13 (P3) `apply_sort_change_reload` の早期 return が、ソート変更以外の「再表示要求」も飲む

**根拠**
- `src/app.rs:20885-20887` は関数の最上段で、Collection root なら無条件に return する。
- この関数はソート変更以外からも呼ばれる:
  - `src/ui_dialogs/batch_convert.rs:369` — 7z/RAR→ZIP 変換後の一覧再構築 (コメントも「一覧を再構築して
    新しい zip / 同名 zip 優先の dedup を反映する」と明記)。
  - `src/ui_dialogs/preferences.rs:2572` — `grid_display_order` 変更時。
  - `src/app.rs:67660` — お気に入りビュー state の復帰時。

**失敗シナリオ (推定、到達可能性は未確認)**
コレクション直下でアーカイブ項目をチェックしてバッチ変換すると、変換後に一覧が更新されない可能性がある。
コレクション直下からバッチ変換ダイアログへ入れるかどうかは確認していない。
`grid_display_order` / お気に入り復帰は、コレクションの並びが DB 定義側の所有なので
何もしないのが正しく、実害は無いと見る。

**修正方向**
「ソート変更後の再表示」と「一覧の再構築要求」を別関数に分け、後者は Collection root では
`schedule_collection_grid_snapshot()` 相当へ振る。最小対応としては、batch_convert の呼び出しだけ
Collection 分岐を足す。

---

### RA-14 (P3) テストの欠落

**A-1 の核心が handler-level テストで固定されていない**
- `src/app/collection_grid.rs:2373` のテストは `start_collection_grid_content_action` を直接呼ぶので、
  「ツールバー / 表示メニューのクリック分岐が `settings.sort_order` を書かない」という A-1 の核心は
  コード読解だけで担保されている。`src/ui_main.rs` にはコレクションのソート UI を通すテストが無い
  (同ファイルのコレクション関連テストは `:22489` と `:22504` の 2 件で、いずれもソート非関連)。
- README §2.2 (c) が求めたのは handler-level テストなので、未達。

**シャッフルで未カバーのもの**
- 再選択で seed が**実際に変わる**ことを誰も検証していない。`collection_store/tests.rs:758-817` は
  revision と catalog revision しか見ないので、`new_shuffle_seed()` (`src/collection_store/db.rs:687-690`) が
  定数を返しても全テストが通る。
- Shuffle → Manual → Shuffle で seed が保持され同じ順に戻ること (`db.rs:289` の書き戻し) が未検証。
- Shuffle 中に entry を追加 / 削除して残りの相対順が不変であること、が未検証
  (`tests.rs:1253-1256` は同一集合の並べ替え不変性まで)。
- Remote と PC の有効順一致 (wire を通した順序の同値性) が未検証。summary enum とラベルまで。

**Auto 比率 cache で未カバーのもの**
- 削除時の掃除 (機能自体が無い、RA-6)。
- actor 不調時にキャッシュ管理ダイアログの件数行が消え、全件削除が「一部削除しました。」になること
  (worker 層の `CacheMaintResult` までしかテストが無い)。
- `begin_maintenance` の送信失敗で epoch が進まず in-memory `entries` も消えない分岐。

---

### RA-15 (P3, Remote) Web の order 分岐が非網羅で、未知の kind が無警告で「手動順」に落ちる

**根拠**
- `crates/remote-web/web/app.js:4883-4907` — `standard` / `shuffle` の後に無条件の
  `return { selected: "manual", label: "手動順", ... }`。console 警告も見た目の区別も無い。
- Rust 側は網羅 match なので (`src/remote_ipc/persistent_collections.rs:2023-2036`)、
  `CollectionOrderMode` に 4 つ目を足すと Rust はコンパイルエラーになるが、Web は静かに誤表示する。
- テスト `crates/remote-web/web/app-runtime.test.mjs:227-233` は manual / shuffle / standard の 3 つだけで、
  未知 kind / `order` 欠落を検証していない (shuffle は `.options[0].label` しか見ておらず
  `.selected === "shuffle"` は未検証)。

**評価**
現状はハンドシェイク (exact equality) が版ズレを防ぐので理論上のリスク。将来の追加に対する保険として
default 分岐を可視化するのが望ましい。

---

### RA-16 (P3, 文書) 実装計画の IPC 版記述が古い

`docs/collection-implementation-plan.md:1165` は「Remote IPCは56→57」のままだが、
`8dbad53e5` で wire は 58 (`crates/remote-ipc/src/lib.rs:31`)。
正本の `docs/web-remote-plan.md:1872-1876` は 58 で正しく、56/57 の由来も記載済み。
実装記録としては当時の値で正しいので、誤読を避けるなら「(当該変更時点。現行は正本参照)」を添える程度でよい。

---

### RA-17 (P3) 全コレクション書き出しが `shuffle_seed` を残さない

`src/collection_store/export_all.rs:121-130` は `order_mode` と `standard_sort` を記録するが
`shuffle_seed` を書かない。一方でエントリのパスは有効順 (= シャッフル順) で出力される (`:117-118`)。
Shuffle 中に書き出したファイルからは、その順を再現できない。
§23.8 のバックアップ用途 (「テキスト export だけが可搬手段」) を考えると、seed も残す方が筋が通る。

---

## 2. 確認して問題なしと判断した項目 (再監査防止)

- **`settings.sort_order` を書く 3 つの UI 経路はすべて Collection 分岐で回避している。**
  表示メニュー `src/ui_main.rs:6582-6594`、ツールバー Buttons `:8965-8990`、Dropdown `:9085-9106`。
  `settings.save()` も同じ分岐の内側に移されており、Collection では呼ばれない。
- **`apply_sort_change_reload` の Collection 早期 return は、全分岐より前に置かれている**
  (`src/app.rs:20885-20887`。zip_nav / 検索 / タグ / 評価 / ブックマーク / スマートフォルダ /
  サブフォルダ展開 / 物理再読込 のどれにも落ちない)。
- **PhysicalSource 子では従来どおり global ソートが効く。** `collection_grid_root_order()` は
  `CollectionGridPosition::Root` 以外で `None` を返す (`src/app/collection_grid.rs:419-421`) ので、
  ツールバーは `settings.sort_order` を表示・書き込み、`apply_sort_change_reload` は物理再読込へ進む。
  テスト `collection_grid.rs:2455-2485` / `:2554-2569`。
- **`open_collection_grid` と履歴復帰の双方で列ソートが戻る。** `src/app/collection_grid.rs:1054-1057`、
  履歴経路は `src/app.rs:19528` `:23741` `:75016` `:75241` がいずれも `open_collection_grid` を通る。
- **同一 root の order 切替でも列ソートが戻り、items と新 order の公開後に details 行順を再構築する。**
  `src/app/collection_grid.rs:1778-1785` (リセット) と `:1922-1928` (再構築)。
  order が変わらない通常の watch 更新では列ソートを保つので、外部更新で利用者の列ソートが消えない。
- **`SetOrder` はクリック時点の revision と完全一致でなければ拒否される。**
  `requires_exact_root_revision()` (`src/ui_dialogs/collections.rs:394` 相当) を使った
  `:2296-2297` の gate。stale クリックはトーストで取り消される。
- **`set_order` は同値 Manual / Standard で revision を進めない。Shuffle だけ常に再 seed + revision 前進。**
  `src/collection_store/db.rs:270-294`。actor の `mutated` も revision 変化を見る (`runtime.rs:960-964`)。
- **シャッフル順の実装は 1 箇所。** `effective_collection_order` (`src/collection_store/model.rs:389-417`) を
  `prepare_collection_snapshot_while` (`src/collection_store/prepare.rs:526`) だけが呼び、
  Grid (`collection_grid.rs:153`) / navigation (`collection_navigation.rs:3356`) /
  Remote (`remote_ipc/persistent_collections.rs:1028`) / export (`prepare.rs:391`) が共有する。
  Remote 側に独自の並べ替えは無く、`exact.prepared.entries` を順序どおり読むだけ。
- **digest の入力バイトが仕様どおり。** seed は `to_le_bytes()` 8 bytes を先に (`model.rs:399` `:405`)、
  entry ID は `as_uuid().as_bytes()` raw 16 bytes (`:406`)、digest 辞書順昇順 (`:411-412`)、
  同値時 UUID raw bytes 昇順 (`:413`)。
- **seed の永続形式が u64 全域を往復する。** `format!("{:016x}")` (`db.rs:287` `:289`)、
  読みは `u64::from_str_radix(..,16)` で失敗時 typed error (`db.rs:825-831`)。
  `u64::MAX` の往復をテストが固定 (`collection_store/tests.rs:808-816`)。
  seed の乱数源は `Uuid::new_v4()` = getrandom 由来の CSPRNG (`db.rs:687-690`)。
  ただし v4 の version nibble が下位 8 バイトに入るため、生成される seed は実質 60 bit
  (保存は 64 bit 往復する)。シャッフル品質としては問題なし。
- **手動 position は order 切替で無傷。** `set_order` は `collections` 行しか触らない (`db.rs:278-294`)。
  Standard / Shuffle 中の追加も `MAX(manual_position)+1` で manual tail へ入る (`db.rs:310-315`)。
  Manual 以外での `ReorderManual` は `ManualOrderInactive` で拒否 (`db.rs:415-417`)。
- **v1→v2 移行が安全側。** 単一 transaction (`db.rs:677-685`)、移行前バックアップの失敗は
  `?` 伝播で移行を中止し v1 を無改変で残す (`db.rs:90-112`)、version は read-only 接続と
  書込接続の 2 回読んで不一致を拒否 (`:53-65` `:75-83`)、将来版は `IncompatibleSchema` (`:64` `:76-78`)、
  DROP / truncate / 空 DB 置換の経路は無い。移行前に READ_ONLY で `integrity_check` +
  全行 parse + `foreign_key_check` (`db.rs:599-636`)。
- **latest-next reducer は変更されていない。** `540fdc216` は reducer に触れておらず、
  有効順が変わるだけ。専用並べ替え画面は Manual のときだけ開く (`collections.rs:2559-2561`)。
- **IPC 版定数は 1 箇所で、Web は版番号を一切持たない。** `crates/remote-ipc/src/lib.rs:31`。
  `crates/remote-web/src/ipc_client.rs` / `src/remote_ipc/pipe.rs` は記号参照のみ。
  Web はエラーコード `"protocol_version_mismatch"` に反応するだけ (`app.js:8826`)。
- **版不一致は最初のフレームで切断され、v56 フレームが v58 デシリアライザに届かない。**
  `src/remote_ipc/pipe.rs:2542-2566` (受理前に `return`)、client 側 `ipc_client.rs:1820-1832`、
  core は child の stderr から再検出して UI に出す (`src/remote_ipc/service.rs:519-524`、
  回帰テスト `:602-613`)。
- **wire の order enum は名前タグで、index 変更のリスクが無い。**
  `crates/remote-ipc/src/lib.rs:1343-1353` (`#[serde(tag="kind", rename_all="snake_case")]`)。
  `#[serde(other)]` が無いので未知 kind は Rust 側では hard error。
  core→wire は wildcard 無しの網羅 match (`src/remote_ipc/persistent_collections.rs:2023-2036`)。
- **Remote からコレクションの並び順は変更できない。** Web の `<select>` は
  `locked_reason` で常に無効 (`app.js:6700`、`changeGridSortOrder` は `:6726` で bail)、
  `gridSortScope = null` (`:5118`)、remote-web 側に `set_sort_order` ハンドラが存在しない。
  core の `persist_remote_sort_order` (`src/remote_ipc/ui.rs:2654`) は通常フォルダ用で、
  コレクション root から到達する導線は見つからなかった。
- **ソート popup の style は局所適用で、メインテーマへ漏れない。**
  `src/ui_main.rs:1586-1597` は `ComboBox::popup_style(StyleModifier)` を使い、
  `ctx.set_visuals` / `ctx.set_theme` を呼ばない。既定の `menu_style` を先に適用してから
  floating scrollbar の予約幅だけ広げるので、ComboBox の見た目も保たれる
  (テスト `src/ui_main.rs:23821-23868` が `button_padding.x == 2.0` と global 0.0 を固定)。
- **「反映待ち」は fullscreen を閉じれば解消する。** 保留は `fullscreen_idx.is_some()` のときだけ成立し
  (`src/app/collection_grid.rs:1309-1311`)、閉じると `collection_grid_root_materialize_active()` が真になって
  poll が再開する (`:1274-1280`)。`collection_grid_poll_delay` が `RequestNeeded` に対して
  `Duration::ZERO` を返すので repaint も駆動される (`:1291-1297`)。
  テスト `collection_grid.rs:4009-4023` が close 後の採用を固定。永久残留の経路は見つからなかった。
- **理由の優先順位が仕様どおり。** Deleted → Failed → viewer-deferred → stale → loading
  (`src/app.rs:52654-52667`)。初回読込は「更新中」、削除・読込失敗が優先される。
- **Auto 比率 cache は UI スレッドで同期 DB アクセスも blocking `recv` もしない。**
  UI スレッドから触るのは `cached()` (HashMap、`auto_aspect_cache.rs:284-286`)、
  `adopt_lookup` (整数比較、`:288-302`)、`record` / `begin_maintenance` (送信のみ、`:317-346`) だけ。
  キャッシュ管理の件数・全件削除は `cache-maint` worker が `recv` し、UI は `try_recv` で回収
  (`src/app.rs:36052`)。
- **100 ms の actor Get は prepare worker 上で行われる。** `src/app/collection_grid.rs:257-263`。
  呼び出し元は 2 つとも `std::thread::Builder::spawn` の内側で、client を渡すのは
  `collection-grid-prepare` だけ (`:1238-1250`)。`collection-navigation-prepare` は `None` を渡す
  (`src/app/collection_navigation.rs:1552`) ので待たない。timeout は単なる cache miss。
- **epoch は clear と Get の受付を同一 lock 内で直列化している。** bump は `:333-341` の critical section、
  Get の stamp は `:377-384` の同じ lock。古い epoch の結果は `adopt_lookup` で棄却 (`:296-299`)。
  in-memory map も clear 時に消す (`:342-344`)。
- **actor 不調でも folder / catalog / tile の処理は続く。** `src/cache_maintenance.rs:255-261` +
  各 match arm。全件削除は「キャッシュを一部削除しました。」に落ちる (`src/app.rs:36121-36125` `:36142-36146`)。
  件数は「不明」の文字列ではなく行を出さない実装 (`src/ui_dialogs/cache_manager.rs:85-87`) で、
  誤った数字は出さないので意図は満たしている (文書の「unknown 表示」とは字面が違う)。
- **`collection_grid_root_order()` は毎フレーム複数回呼ばれるが O(1)。**
  `src/ui_main.rs:5561` `:6106` `:8343` + `grid_sort_lock_reason` / `details_header_sort_active` /
  `details_header_sort_locked` の内側。実体は session / prepared の参照と長さ比較だけ
  (`src/app/collection_grid.rs:382-448`)。走査や clone は無い。
- **`current_reader_order()` のガードが十分。** `installed_items_generation == items_generation`、
  `prepared.collection_id == identity.collection_id`、`collection_revision == accepted_revision`、
  `entries.len() == items.len()` の 4 条件が揃ったときだけ installed 順を使い、
  それ以外は表示順へフォールバックする (`src/app.rs:52561-52581`)。
  stale な install で reader が壊れる経路は見つからなかった。
- **UI 文言に危険グリフ・内部用語の混入は無い。** 新規文字列は
  「手動 / 手動順 / シャッフル / シャッフル（再選択で並べ直す）/ 固定 / 列ヘッダ / 更新中 / 反映待ち /
  更新待ち / 読込失敗 / 削除済み」と各 tooltip。`✓` (U+2713) の使用は表示メニュー・製本メニューの
  既存慣行と同じ。実装語 (revision / snapshot / actor 等) は UI に出ていない。
- **`main_window_title` の Collection 分岐は Root 限定。** `src/app.rs:72752-72767` の
  `filter(|session| matches!(session.position, Root))` により、PhysicalSource 子では
  従来どおり `effective_folder()` のパスが出る。
