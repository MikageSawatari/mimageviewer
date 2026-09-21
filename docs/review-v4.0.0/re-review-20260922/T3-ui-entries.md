# T3 UI 入口の最終再レビュー (v4.0.0 出荷直前)

読み取り専用。アプリ起動・cargo 実行・git 書き込みは行っていない。実行時観測はゼロで、
以下はすべて作業ツリーのコードを読んだ限りの推定。行番号は未コミットの作業ツリー。

## 結論

- **出荷を止める指摘 (P1): 0 件**
- **確度の高い P2: 0 件**
- 対象 8 件のうち 7 件は解消 / 不採用妥当。RA-3 のみ「マニュアルは書かれたが画面表示は未実装」で部分的。
- 利用者の設定やデータを黙って書き換える経路、誤った対象を開く / 登録する / 削除する経路、
  操作不能になる経路は見つからなかった。

## 元指摘の判定

| ID | 判定 | 根拠 |
| --- | --- | --- |
| RA-1 パッドのリングソート | **解消** | 下記 1 |
| RA-2 無効理由が事実と違う | **解消** | `src/app.rs:10264-10288` が typed reason を持ち、`src/ui_main.rs:16053-16054` が `details_sort_lock.tooltip()` を出す。Manual / Shuffle 専用文言あり |
| RA-3 表示順と読み順の乖離 | **部分的** | 下記 2 |
| RA-4 音楽 HUD の前後移動 | **不採用は妥当** | 下記 3 |
| RC-1 英語エラーのトースト | **解消** | 下記 4 |
| S1-1 管理画面の番号 | **解消** | 下記 5 |
| S1-4 「追加先の本を開く」の統一 | **解消 (仕様として統一)** | 下記 6 |
| S1-5 固定本ボタンが一時的に消える | **解消** | 下記 7 |

## 1. RA-1 (解消)

- 3 経路とも対象コレクションへ統一されている。
  - 構築: `src/app/gamepad_input.rs:2996-3016`。コレクション root では `collection_grid_root_order()` の
    `target.standard_sort` を初期値にし、`settings.sort_order` を読まない。
  - preview: 同 `:3846-3851`。`RingPickerGridSortTarget::Global` の場合だけ `apply_grid_picker_sort_order`
    を呼ぶ。Collection は overlay 値だけ動き、actor へ何も送らないので revision は進まない。
  - 確定: 同 `:4926-4943`。Collection は `request_collection_grid_set_order(..., StandardControl)` のみ。
- owner 失効: 確定は `apply_ring_picker_state` (`:4858-4888`) が開いた viewer context を mount し
  anchor 一致を見たうえで、`resolve_collection_grid_set_order` (`src/app/collection_grid.rs:540-579`) が
  **その場で `collection_grid_root_order()` を読み直し**、stamp / expected_revision 不一致なら `NoOp`。
  更新中 / 削除済み / 読込失敗は `collection_grid_root_order()` が `Err(reason)` を返し、
  picker 側は `Locked` になって行自体が編集不可 (`:3720-3726`)。
- 通常フォルダの退行なし: `collection_grid_root_order()` が `None` を返すので `Global` に落ち、
  従来どおり `settings.sort_order` を書いて保存する。ただし本表示 / 閲覧履歴 / 列ヘッダソート中は
  `grid_sort_lock_reason()` により Locked になる。これはツールバー・表示メニューと同じ述語で、
  以前ツールバーだけ無効だった不整合が揃った形。
- `settings.sort_order` の書き込み元を全列挙した結果、コレクション root から到達できる未対処経路は無い。
  ツールバー 2 箇所 (`src/ui_main.rs:9170-9194`, `:9296-9317`) と表示メニュー (`:6762-6772`) はいずれも
  `root_order` 分岐が先にあり、かつ `sort_lock` で widget 自体が無効化される
  (`ui_main.rs:9160` の `add_enabled(!sort_disabled, ..)`、`:9246` の `toolbar_combo_slot(ui, !sort_disabled, ..)`、
  `:6716-6726` は `sort_lock` が Some ならサブメニューを描かない)。
- Remote: `persist_remote_sort_order` (`src/remote_ipc/ui.rs:2654-2670`) は global を書くが、
  コレクション閲覧中の Web UI からは到達しない。保存コレクションのルートは
  `crates/remote-web/web/app.js:5117-5118` で `gridSortScope = null` を置き、並べ替えバーは
  `persistentCollectionSortState` (`:4883-4903`) が `locked_reason` 付きの読み取り専用状態を返す。
  `changeGridSortOrder` (`:6725-6726`) は scope が null か locked なら即 return する。

## 2. RA-3 (部分的 — 出荷は止めない)

利用者が選んだ案は「仕様は維持し、列ヘッダ所有中に『列の並びは一覧表示だけ〜』を hover か
ヘッダ行に出し、マニュアルへ明記」だった。**マニュアルだけ入っている。**

- マニュアル: `htdocs/mimageviewer/manual/collections.html` の「通常ソート」行が
  「一覧の表示と選択だけに使います。画像の前後移動・Home / End・見開き・スライドショー、
  動画や音声の前後／連続再生、書き出し、リモート閲覧は、保存されたコレクションの
  有効な順番を保ちます」に書き換わっている。
- 画面側: 該当文言は `src/` に存在しない (`並び順に従`・`有効な順番`・`表示と選択だけ` の全文検索で 0 件)。
  列ヘッダ所有中に出るのは、ツールバー / メニューの無効理由
  `GridSortLockReason::DetailsHeaderSort` の汎用文言 (`src/app.rs:10269-10271`) だけで、
  送り順がコレクション順に従うことは画面上では分からない。
  `details_header_title` (`src/ui_main.rs:15395-15430`) にも注記は無い。
- 影響は「1 行目を開いて → を押すと 2 行目でない項目が出る」という既知の仕様どおりの挙動で、
  データは壊れない。出荷を止める必要は無いと判断する。文言を 1 つ足すなら
  `ui_main.rs:16051-16059` の hover か、`root_order` が Some かつ `details_header_sort_active()` の
  ときのヘッダ行が最小の置き場所。

## 3. RA-4 (不採用は妥当)

- `music_navigate_file` (`src/ui_music_panels.rs:344-352`) は `current_grid_order()` を渡すが、
  `start_manual_media_navigation` (`src/app.rs:72085-72087`) の冒頭で
  `start_collection_manual_navigation` が true を返した時点で return し、`display_order` は
  一切参照されない。
- `start_collection_manual_navigation_inner` (`src/app/collection_navigation.rs:1201-1246`) が
  true を返す条件は「継続中の pending がある」か「`collection_root_navigation_origin(fs_idx, true)`
  が Some」。正常な Collection root ではこれが成立するので、reader 順で次が選ばれる。
- 手動ナビの入口を全列挙した (`ui_fullscreen.rs:33913 / 34002 / 34070`、`native_video.rs:14048 / 14073 / 14113`、
  `ui_music_panels.rs:345`)。いずれも `start_collection_manual_navigation(_display_unit)` を
  先に通る。Collection 以外では reader 順 = grid 順なので差は出ない。
- 音声は見開き partner を持たないので、キーボード側の `display_unit` 版との差も出ない。

## 4. RC-1 (解消)

- `finish_collection_navigation_store_error` (`src/app/collection_navigation.rs:1169-1179`) が
  `error.user_message()` を渡す。`user_message()` は全 variant が日本語
  (`src/collection_store/model.rs:369-389`)。英語の `Display` は `logger::log` 行だけに残る。
- 他の入口 (`:1033`, `:1058`, `:1861`, `:1910`) も固定の日本語文字列。
- 管理ダイアログ側も `collection_error_message` = `user_message()` (`src/ui_dialogs/collections.rs:1244-1246`)。
- 残る「日本語の前置き + OS / serde の英語」形式は **6 箇所** (`ui_dialogs/collections.rs:319, 3176, 4102, 4106, 4112`、
  `app/collection_grid.rs:1510`)。いずれも io / serde の原因表示で、P3 として据え置きでよい。

## 5. S1-1 (解消)

- **番号キーの対象と画面の番号は同一配列・同一順序から導かれている。**
  - コレクション: 管理画面は `self.collection_ui.catalog` (`src/ui_dialogs/collections.rs:3716`) を
    `definitions` 順に列挙 (`:3822`)。キー側の `collection_action_catalog`
    (`:1977-2011`) も同じ `catalog.definitions` から `ids` を作り、
    `collection_action_target` が `ids.get(slot - 1)` を引く (`src/app/saved_group_actions.rs:99-104`)。
    DB も `ORDER BY catalog_position ASC` で一貫 (`src/collection_store/db.rs:1134, 1232`)。
  - 本: 管理画面は `self.book_list_cache` を列挙 (`src/ui_main.rs:7242`)、キー側も同じ
    `book_list_cache` に `rows.get(slot - 1)` (`src/app/saved_group_actions.rs:64-71`,
    `:398`, `:526`)。別々に数える余地は無い。
- 21 件目以降: `saved_group_slot_number(index)` が `index >= 20` で None
  (`src/ui_main.rs:49`, テスト `:23351-23354`)。両画面とも「—」+ hover
  「番号操作の対象外です（1〜20のみ）」(`ui_main.rs:7247-7252`, `ui_dialogs/collections.rs:3831-3836`)。
- 変動条件の説明: 列見出しの hover にコレクション「作成順で、削除すると後ろの番号が繰り上がります」
  (`collections.rs:3797-3800`)、本「名前順のため、追加・名前変更・削除で番号が変わります」
  (`ui_main.rs:7214-7217`)。マニュアル (collections.html / books.html / shortcuts.html /
  tut-books.html / tut-collections.html) の記述も同じ規則で一致している。
- 管理画面が更新中の一瞬だけ、画面の番号 (旧 catalog) とキーの対象 (更新後 catalog を待って開く) が
  食い違い得るが、キー側は `collection_action_catalog` が更新中に Err を返して待つ構造なので、
  **古い番号で誤った対象を開くことはない**。
- スナップショットは `tests/snapshots/book_manager_numbered_dark.png` が新規、
  `collection_manager_populated_dark.png` が更新済み。

## 6. S1-4 (解消。ただし warm でも待機モーダルは出る)

- メニュー / ツールバー / トレイ / キーがすべて `open_active_book_from_action`
  (`src/app/saved_group_actions.rs:132-134`) → `apply_saved_group_key_action(GridOpenActiveBook)`
  へ収束 (`src/ui_main.rs:6217`, `:9840`, `src/tray_integration.rs:721`)。
  旧実装のメニューは `fav_nav = Some(self.active_book_folder_path())` で即時遷移していた
  (`git show HEAD:src/ui_main.rs` の `MenuCommandId::BooksOpenActiveBook`)。
- **本は cache が温かくても待機モーダルが 1 フレーム以上出る。** `GridOpenActiveBook` は
  `book_list_cache` を使わず必ず `start_saved_book_scan` を投げ (`saved_group_actions.rs:390-391, 418-424`)、
  scan は worker スレッド。`App::update` はメニュー描画 `src/app.rs:74733` の直後に
  `render_saved_group_open_modal` `:74739` を呼び、結果の poll は次フレームの `:74082` なので、
  メニュー経由では同一フレームに「本を開く準備中...」が出る。
  これは台帳が明示的に選んだ統一方針で、マニュアル (`books.html`, `shortcuts.html`) にも
  「キー、メニュー、ツールバーの『追加先の本を開く』は同じ待機画面を使い、『中止』または Esc で
  取り消せます」と書かれている。**不具合ではなく承認済みの仕様変更**と判断する。
- コレクション側は非対称で、catalog が Ready なら `collection_action_catalog()` が即 Ok を返し
  モーダル無しで開く (`saved_group_actions.rs:432-452`)。本はフォルダ走査が必要なので
  この差は妥当。
- 取消: モーダルに「中止」ボタンと `dialog_escape_pressed` (`:291-304`)。
  タイムアウトは日本語トースト (`:490-497`)。トレイ格納は
  `root_tray_hide_modal_owner` が `saved_group_open` を含むので保留される
  (`src/tray_integration.rs:255-263, 629-648`) — S1-2 も解消。

## 7. S1-5 (解消)

- `open_book_manager_from_action` (`src/app/saved_group_actions.rs:125-129`) は
  `show_book_manager = true` と `request_book_list_refresh()` だけで、`book_list_cache` を消さない。
  `request_book_list_refresh` (`src/app.rs:35586-35608`) も worker を投げるだけで cache に触れない。
- 旧コードの `MenuCommandId::BooksManage` にあった `self.book_list_cache = None;` は消えている。
  ツールバーの固定本ボタンは `book_list_cache` の存在で描くので、管理画面を開いても消えない。
- 残る `book_list_cache = None` は、並べ替え画面を開くとき (`ui_main.rs:7154`)、
  管理画面の「更新」ボタン (`:7385`)、ページ編集の反映後 (`:7521`)、本操作の完了時
  (`app.rs:36131` ほか) で、いずれも一覧が実際に変わる契機。

## 8. マニュアル照合

未コミット差分を実装と突き合わせた範囲で、重要な不一致は無い。

- 番号の意味 (コレクション = 作成順 / 本 = 名前順、固定ボタンではなく管理一覧の全件、21 件目以降は無し、
  並べ替え UI は無い) は UI の hover と一致。
- 待機の取消 (「中止」または Esc)、長時間時の案内で待機終了、PDF パスワード入力中は
  待ち時間に含めない、待機中は閉じてもトレイへ格納しない — いずれも対応コードを確認した。
- 「元の場所へ移動」の親なし無効化、取り込みの容量拒否表示、書き出しの UTF-8 BOM も
  マニュアル側の記述が実装の方向と矛盾しない。
- RA-3 だけ、マニュアルが画面より先に進んでいる (上記 2)。マニュアルの記述自体は仕様として正しいので、
  修正するなら画面側を足す方向。
