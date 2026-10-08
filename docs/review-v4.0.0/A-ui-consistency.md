# レビュー A: コレクション機能の UI / 操作体系の整合性監査 (v4.0.0 出荷前)

対象: `master` @ `0a7139d27` (Cargo.toml version = 3.10.0、v4.0.0 未採番)
範囲: 「UI の作り・操作体系が既存機能と整合しているか」。Remote 側 UI は別担当。
方法: ソース読解のみ。**アプリは起動していない。実行時の挙動はすべて「コードを読んだ限りの推定」**で、
実機での観測ではない。`python scripts/check_ui_glyphs.py` のみ実行 (read-only、0 件 / exit 0)。

## 要約

| 重要度 | 件数 | 内訳 |
| --- | --- | --- |
| P1 | 2 | A-1 ソート入口の空振り + グローバル設定汚染 / A-2 列ヘッダソートが手動順を黙って上書き |
| P2 | 5 | A-3 / A-4 / A-5 / A-6 / A-7 |
| P3 | 11 | A-8 〜 A-18 |
| 設計提案 | 1 | D-1 (A-1 の修正方向: 手動順をソート選択肢へ統合する案) |

最も重い所見は 2 件とも **「コレクションの並び順を誰が所有するか」が UI 層で決まっていない**ことに
帰着する。`TopLevelGridSurface::Collection` は surface enum だけで表現され、`items_are_*` 相当の
述語を持たないため、並び順・タイトル・親移動・ジャンプなど「合成一覧なら分岐する」場所の大半で
**`current_folder == None` の物理フォルダ**として扱われ、分岐そのものに到達していない。

`current_folder` が `None` である事実は `src/app/collection_grid.rs:1487` が唯一の根拠で、
他の合成 surface (Bookmarks / Rating / ReadingHistory / SubfolderExpansion / SmartFolder) は
`start_loading_items(<synthetic path>)` 経由で `current_folder = Some(合成パス)` を持つ
(例: `src/app.rs:33712-33720`)。この差が、下記の各指摘の「安全側に倒れた」ものと
「危険側に倒れた」ものの両方を生んでいる。

---

## P1

### A-1 (P1) コレクション直下でツールバー / 表示メニューのソートが「選べるのに効かず、しかも全フォルダ共通の設定を黙って書き換える」

**根拠 (file:line)**
- `src/app.rs:32938-32959` `page_order_locked_for_current_view()` — 冒頭で
  `let Some(current_folder) = self.current_folder.as_deref() else { return false; }`。
  コレクション root は `current_folder == None` (`src/app/collection_grid.rs:1487`) なので **false**。
- `src/app.rs:51411-51418` `grid_sort_lock_reason()` — 上記が false、`details_header_sort_active()` も
  既定 (`DetailsSortKey::Toolbar`) では false → **`None` = ロックなし**。
- `src/ui_main.rs:8817` `let sort_disabled = sort_lock.is_some();` → ソートボタン / コンボは有効。
- `src/ui_main.rs:8838-8873` 選択状態の算出は
  `if items_are_bookmark_view {…} else if items_are_rating_view {…} else { self.settings.sort_order == order }`。
  Collection 用の分岐が無いため **`settings.sort_order` が「現在の並び」として表示される**。
- `src/ui_main.rs:8862-8873` クリック時: `self.settings.sort_order = order; self.settings.save();`
  → Collection 分岐が無いので `toolbar_sort_changed = true` へ落ちる。
- `src/ui_main.rs:9496` → `src/app.rs:20502 apply_sort_change_reload()`。
  `src/app.rs:20511-20578` の分岐列 (zip_nav / global_search / tag_view / rating / bookmark /
  smart folder / subfolder expansion) に Collection は無く、
  `src/app.rs:20579` `if let Some(path) = self.current_folder.clone()` が **None なので完全な no-op**。
- 表示メニュー側も同じ: `src/ui_main.rs:6493-6528`。無効化は `sort_lock` のみ、checked は
  `items_are_rating_view` 以外すべて `settings.sort_order`、クリックは `sort_changed = true`。
- 一方、コレクションの実際の並びは `CollectionDefinition { order_mode, standard_sort }`
  (`src/collection_store/model.rs:186`) が所有し、`src/ui_main.rs:6162-6205` の
  上部「コレクション」メニュー →「現在のコレクションの並び順」→ 手動順 / 通常ソート▸ でしか変えられない。
  `standard_sort` の型は `settings::SortOrder` と同一。

**コード上の現状 (推定される利用者体験)**
1. 手動順のコレクションを開くと、ツールバーには `settings.sort_order` (直前に見ていた実フォルダの
   並び、例「名前↑」) が選択済みとして表示される。**実際の並び (手動順) はどこにも表示されない。**
2. そのボタンを押しても一覧は変わらない (`apply_sort_change_reload` が no-op)。
3. それでも `settings.sort_order` は書き換わり `save()` される。**コレクションを閉じて実フォルダへ
   戻ると、全フォルダ共通のソート順が知らないうちに変わっている。**

利用者報告 (「コレクションの並び順を手動にしても、ツールバーのソートが本棚のように固定にならず、
ソート順を変えても変更されない」) はこの経路と一致する。3 番目の副作用は報告に含まれていないが、
コード上は必ず起きる (推定)。

**揃えるべき既存の振る舞い**
- 本 (製本 / ZIP / PDF / 閲覧履歴) は `page_order_locked_for_current_view()` → `PageOrderFixed` で
  ツールバー / メニュー両方を無効化し、コンボに「固定」、ツールチップに理由を出す
  (`src/app.rs:10011-10040`, `src/ui_main.rs:8819-8824`, `8908-8915`, `6493-6503`)。
- レーティング / ブックマーク一覧はビュー専用のソート型を持ち、`Normal(SortOrder)` と
  時刻順を同じコンボへ並べる (`src/ui_main.rs:8876-8905`, `src/app.rs:20526-20547`)。
- `src/app.rs:51408-51410` のコメントが明示している通り、**「ツールバーもメニューも必ずこの 1 つの
  述語を見る」**のが §1.143(a) の設計。Collection がその述語に入っていないのが欠落そのもの。

**修正方向** → D-1 を参照。

---

### A-2 (P1) 詳細表示の列ヘッダソートがコレクションの手動順を黙って上書きする (§1.143(a) と同型)

**根拠 (file:line)**
- `src/app.rs:51399-51404` `details_header_sort_active()` =
  `grid_view_mode == Details && !current_folder_is_book_folder() && !items_are_reading_history_view
   && settings.details_sort_key != DetailsSortKey::Toolbar`。
  コレクション root では `current_folder_is_book_folder()` が `current_folder == None` により false
  (`src/app.rs:32912-32917`)、`items_are_reading_history_view` も false → **列ソートが有効**。
- `src/app.rs:51532-51563` `rebuild_details_order()` は `visible_indices` を汎用に並べ替えるので、
  コレクションの手動順 (= prepared snapshot の entry 順で install された `items`) を上書きする。
- `settings.details_sort_key` は**永続設定**であり、フォルダをまたいで持ち込まれる。
- `reset_details_sort_to_toolbar()` の呼び出し元は 4 箇所のみ:
  `src/app.rs:19881` (Rating 履歴復帰) / `19930` (Bookmarks 履歴復帰) / `24026` (`enter_rating_view`) /
  `33677` (`open_bookmark_browser`)。**`open_collection_grid` (`src/app/collection_grid.rs:857`) には無い。**

**揃えるべき既存の振る舞い**
`src/app.rs:51419-51426` のコメントが規約を明記している —
「ビューの既定ソートを入れ直すのと同じ場所で戻し、以後その一覧で列ヘッダを押したときだけ列ソートが
所有権を取る」。Rating / Bookmark はこれに従っているが Collection は従っていない。

**想定される利用者の困り方 (推定)**
実フォルダで詳細表示の「サイズ」列ヘッダを押して並べ替えたまま、ツールバーの「開く」で手動順の
コレクションを開くと、**手動で並べた順が消えてサイズ順で表示される**。原因表示は無く、
上部「コレクション」メニューの「並び順」は「手動順」のままなので、利用者からは矛盾に見える。

**修正方向**
`open_collection_grid` (および履歴からの Collection 復帰 `src/app.rs:19868` 付近) で
`reset_details_sort_to_toolbar()` を呼ぶ。加えて A-1 / D-1 の `page_order_locked` 側で
手動順を `PageOrderFixed` 相当にすれば、`grid_sort_lock_reason` → `DetailsHeaderSort` より
先に `PageOrderFixed` が勝ち、列ヘッダソートも `sort_enabled = false`
(`src/ui_main.rs:15638`) で無効化されて二重に守れる。

---

## P2

### A-3 (P2) 一覧更新中の右クリックで「コレクションから外す」だけが消え、「元ファイルをゴミ箱へ移動」が残る

**根拠 (file:line)**
- `src/ui_dialogs/context_menu.rs:1351-1352`
  `collection_reference: target.collection_root_delete.ready_target().is_some()`,
  `collection_source_context: target.collection_root_delete.is_collection_root()`。
- `src/app/collection_grid.rs:369-408` `collection_root_delete_resolution()` は、session 不在 /
  stamp 不在 / `installed_items_generation != items_generation` / prepared 不在 /
  `prepared.collection_revision != accepted_revision` / `entries.len() != items.len()` のいずれかで
  `Unavailable("コレクション一覧を更新中のため、登録解除できません")` を返す。
  この状態では `ready_target()` が None なので **`collection_reference == false`、
  `collection_source_context == true`**。
- `src/context_menu_model.rs:1427-1438` — `RemoveFromCollection` は
  `input.collection_reference` が true のときだけ push される → **項目ごと消える**。
- `src/context_menu_model.rs:1441-1455` — `MoveToRecycleBin` は
  `input.kind.supports_delete()` だけを見るので残り、ラベルだけが
  `collection_source_context` によって「元ファイルをゴミ箱へ移動 (タグ・評価も整理)」になる。
- `src/ui_dialogs/context_menu.rs:1240-1249` — `delete_targets` は
  `item.drag_source_path()` から独立に作られるので、実際に実行できてしまう。

**揃えるべき既存の振る舞い**
Delete キー経路は fail closed になっている (`src/ui_dialogs/context_menu.rs:2093-2104`:
`Unavailable(reason)` → `show_feedback_toast(reason)` して `return`、物理削除へ落ちない)。
仕様も「読み込み中など参照 binding を確定できない時は何も削除せず通知する」と定めている
(`docs/collection-spec-proposal.md`「実装前に固定する境界」)。
**同じ状態で右クリックメニューだけが安全側の選択肢を消し、危険側だけを残すのは非対称。**

**想定される利用者の困り方 (推定)**
コレクションへ項目を追加した直後 (watch 更新の適用中) に別項目を右クリックすると、
「コレクションから外す」が無く「元ファイルをゴミ箱へ移動…」しか無い。登録解除のつもりで
選ぶと元ファイルがゴミ箱へ行く。発生窓は短いと推定されるが、失う物が元ファイルなので
重みが違う。

**修正方向**
`CollectionRootDeleteResolution::Unavailable(reason)` のとき、`RemoveFromCollection` を
**無効項目 + `disabled_reason = reason`** として push する (`MenuNode::Item { enabled, disabled_reason }`
は既に表現力がある: `src/context_menu_model.rs:1182-1189` に前例)。
`ContextMenuInput` に `collection_reference_unavailable: Option<String>` 相当を足し、
`Grid` 面の checked / 単一の両方へ反映する。

---

### A-4 (P2) キーボード / リング / ゲームパッドからコレクションを操作する入口が一切ない (本棚は 4 系統ある)

**根拠 (file:line)**
- 本棚の `KeyAction`:
  `src/keymap.rs:1561 GridAddToActiveBook` (既定 `Ctrl+B`: `src/keymap.rs:5715`)、
  `1633 FsAddToActiveBook` (`5787`)、`1784 VideoAddToActiveBook` (`5942`)、
  `1491 GridOpenLocationBooksRoot`。
- フルスクリーン実装: `src/ui_fullscreen.rs:28752-28754`
  (`add_fullscreen_image_to_active_book`)、`src/ui_fullscreen.rs:45147`
  (`add_current_video_frame_to_active_book`)。
- マウスリング / ゲームパッドリング: `src/ring_shortcut.rs:539 AddToBook`、
  `src/ring_shortcut.rs:604 OpenLocationBooksRoot`。
- コレクション側: `src/keymap.rs` の `KeyAction` 列挙に `Collection*` は **0 件**。
  `MenuCommandId::Collections*` (`src/keymap.rs:2499-2506`) はすべて `action: None`
  (`src/keymap.rs:2792-2838`)。`src/ring_shortcut.rs` にも collection 系コマンドは無い。
  → 追加 / 開く / 管理はすべて**マウスでツールバーか上部メニューを操作するしかない**。

**揃えるべき既存の振る舞い**
CLAUDE.md「バグ修正の一般原則」末尾:「キー操作を追加・変更するときは、ユーザーが明示していなくても
keymap 対応を検討する。閲覧・編集・動画の通常ショートカットは `KeyAction` に追加し …」。
本棚 (`docs/compile-book-plan.md` が「同型」の参照元) は Grid / Fullscreen / Video の 3 面すべてで
`Ctrl+B` を持つ。

**想定される利用者の困り方 (推定)**
仕様が挙げる主用途は「BGM音楽用、静止画スライドショー用」のプレイリスト。
フルスクリーンで再生 / 閲覧しながら「これも入れておく」ができず、一度全画面を抜けて
グリッドでツールバーを押す必要がある。動画再生中も同じ。

**修正方向**
最低限 `GridAddToCollectionTarget` / `FsAddToCollectionTarget` / `VideoAddToCollectionTarget` の
3 つを `KeyAction` へ追加し、`ini_name()` / `context()` / `trigger()` / `default_chords()` /
`ALL_ACTIONS` / 呼び出し側 helper / `docs/keymap.ini.default` を揃える。既定 chord は未割当 (空) でも
「操作カスタマイズから割り当てられる」だけで大半の要望を満たせる。
固定扱いにするなら、その理由を `docs/keymap-spec.md` へ残す (CLAUDE.md の規約)。

---

### A-5 (P2) コレクション表示中にウィンドウタイトルが「mimageviewer」だけになる

**根拠 (file:line)**
- `src/app.rs:9945-9971` `main_window_title()` は
  reading_history / bookmark / subfolder_expansion / smart_folder_name を専用文言にし、
  それ以外は `effective_folder()` のパス、無ければ `"mimageviewer"`。
- `src/app.rs:18585-18593` `effective_folder()` = `archive_source_override.or(current_folder)`。
  コレクション root はどちらも None (`src/app/collection_grid.rs:1487, 1488`)。
- 呼び出し: `src/app.rs:72655-72663` (Collection を渡す引数が無い)。

**揃えるべき既存の振る舞い**
`閲覧履歴 - mimageviewer` / `ブックマーク - mimageviewer` / `サブフォルダ展開 - mimageviewer` /
`スマートフォルダ: {name} - mimageviewer`。アドレスバーは既に
`コレクション: {name}` (`src/app/collection_grid.rs:1417`) を出しており、同じ素材がある。

**想定される利用者の困り方 (推定)**
複数ウィンドウ / タスクバーのプレビューでどのコレクションを開いているか区別できない。
`mimageviewer` 単体のタイトルは通常「フォルダ未選択 / 起動直後」を意味するので誤解を招く。

**修正方向**
`main_window_title` に `collection_name: Option<&str>` を足し、
`コレクション: {name} - mimageviewer` を返す。(参考: レーティング一覧は
`current_folder` が合成パスのため `…\__rating_view__ - mimageviewer` という生パスが出る既存不具合が
ある。Collection を直す際にまとめて typed 化するのが筋。)

---

### A-6 (P2) 仕様の「元の場所を開く」が未実装 — コレクション項目から登録元フォルダへ mIV 内で移動できない

**根拠 (file:line)**
- 仕様: `docs/collection-spec-proposal.md`「操作案」—
  「『元の場所を開く』は明示的に実フォルダへ移る操作として分離する」。
- `src/context_menu_model.rs:1325-1337` `can_jump_to_folder = input.view.in_search && …`。
- `src/ui_dialogs/context_menu.rs:891-899` `in_search` は
  global_search / drill / favsearch / tag_view のみ。コレクション root では **false**。
- よって `src/context_menu_model.rs:1350` の「フォルダに移動」は push されない。
- `src/app/collection_grid.rs` / `src/app/collection_navigation.rs` に `JumpToFolder` 相当の
  経路は無い (grep 0 件)。
- 残っているのは `MenuCommand::OpenFolderInExplorer`「このフォルダをエクスプローラで開く」
  (`src/context_menu_model.rs:1412-1419`) だけで、これは **外部の Explorer を開く操作**であり
  mIV 内ナビゲーションではない。

**揃えるべき既存の振る舞い**
検索結果ビュー (Ctrl+G / Ctrl+S / Ctrl+T) は「フォルダに移動」で、散在するヒットから
収納フォルダへ飛べる (`src/ui_dialogs/context_menu.rs:1782-1790` に設計コメント)。
コレクションは「離れた場所の実ファイルを一つの一覧にする」機能なので、同じ動線が要る。

**想定される利用者の困り方 (推定)**
コレクション内の 1 枚を見て「この画像の兄弟が見たい」と思っても、mIV 内では辿れない。
フォルダ項目ならダブルクリックで中へ入れるが、**画像 / 動画項目には手段が無い**。

**修正方向**
`ContextMenuViewFlags` に `collection: bool` を足すか、`can_jump_to_folder` の条件へ
`input.collection_reference` を OR する。移動先は既存の `JumpToFolderRequest`
(`src/ui_dialogs/context_menu.rs:68`) をそのまま使い、`selection` に元ファイルを渡す。
遷移時は独立ナビゲーション扱いなので `docs/top-level-grid-view.md`「コレクションrootと物理子のowner」
に従い、visible load 採用時に Collection surface を退役させる。

---

### A-7 (P2) 利用者向け文書にコレクションの独立した説明が無い

**根拠 (file:line)**
- `htdocs/mimageviewer/index.html` — 「コレクション」の出現 **0 件**。
- `htdocs/mimageviewer/manual/` — 独立ページ無し。実質的な説明は
  `manual/settings.html:197, 205-207, 211, 487, 496` の**ツールバー / 設定節のみ**。
  `manual/grid.html` は 0 件、`manual/tutorial.html` は一般名詞としての「コレクション」2 件のみ。
- 比較: 同型とされる本棚は `manual/books.html` を持つ。
- `src/version_highlights.rs:853` の `TABLE` 最新エントリは `"3.10.0"`。v4.0.0 の
  `must_read` / `highlights` が未記入 (A-16 も参照)。

**揃えるべき既存の振る舞い**
CLAUDE.md「コード修正時のドキュメント同時更新」— `manual/` と `index.html` を同時に更新する。
リリース手順 Phase 1 の 6 で「新機能がマニュアル・製品ページに反映されていることを確認」。
サイドバーを持つ通常ページは全ページでリンク数を揃える必要がある (Phase 1-6 のチェック)。

**修正方向**
`manual/collections.html` を新設し、サイドバーを持つ全ページのリンク一覧へ追加
(追加後はページ数が変わるので Phase 1-6 のカウント基準も更新)。`index.html` の機能一覧へ追記。
`manual/shortcuts.html` は A-4 の結論が出てから更新する。

---

## P3

### A-8 (P3) 管理 window / 専用並べ替え window に `default_pos` が無い
`src/ui_dialogs/collections.rs:2577-2583` (コレクションの管理) と `3259-3272` (コレクション並べ替え) は
`.open()/.default_size()/.min_size()` のみで `default_pos` を指定していない。
比較: `src/ui_main.rs:6926-6929` (製本の管理) は `ctx.content_rect().center() - vec2(250,220)`、
`src/ui_dialogs/smart_folder_editor.rs:414`、`about.rs:26`、`cache_manager.rs:50`、
`tag_editor.rs:50`、`favorites_editor.rs:273` などすべて `default_pos` を持つ。
CLAUDE.md「ダイアログ (egui::Window)」の「必ず `default_pos()` を使う」規約に従っていない。
なお `src/ui_dialogs/collections.rs:2812` (コレクション処理) は規約どおり
`ctx.content_rect().min + vec2(60,40)` を使っており、同じファイル内でも不統一。

### A-9 (P3) 「現在のコレクションの並び順」が無効時に理由を出さない / ✓ 表記が他メニューと不統一
`src/ui_main.rs:6162-6163` は `ui.add_enabled_ui(current_target.is_some(), |ui| ui.menu_button(...))`。
`menu_button` を `add_enabled_ui` で包むと灰色になるだけで `on_disabled_hover_text` が付かない。
同じメニュー内の兄弟 (`src/ui_main.rs:6126-6128`, `6144-6146`, `6220-6224`) はいずれも
「コレクション直下を開くと使用できます」等の理由を出している。
また項目のチェック表現が `selectable_label` (`6167-6171`, `6186-6191`) で、
表示メニュー / 製本メニューの `"✓ "` プレフィックス + `ui.button` (`src/ui_main.rs:6512-6516`,
`6058-6062`) と食い違う。

### A-10 (P3) 「場所▼」メニューにコレクションが無い (本棚はある)
`src/known_folders.rs:22-31` `LocationMenuEntry` は
`DriveList / ReadingHistory / Bookmarks / Rating / Bookshelf / Separator / QuickLocation / DriveRoot`。
Collection variant が無く、`src/known_folders.rs:66-69` の `Bookshelf` に相当する行も無い。
対応する `KeyAction::GridOpenLocationBooksRoot` (`src/keymap.rs:1491`) と
`RingCommand::OpenLocationBooksRoot` (`src/ring_shortcut.rs:604`) にも collection 版が無い。
仕様が「場所一覧を主な入口にする案は置き換える」としているので意図的な可能性はあるが、
本棚が残っている以上は非対称。判断が要る (この型は Remote Home とも共有される)。

### A-11 (P3) Ctrl+F の絞り込みが watch 更新のたびに黙って消える
`src/app/collection_grid.rs:1503-1504` `install_collection_grid_items_with_thumbnail_sources` は
`self.search_filter = None; self.search_query.clear();` を無条件に行うが `show_search_bar` は触らない。
コレクション root は revision 更新 (他窓からの追加、Remove 完了など) のたびに再 install され得るので、
**絞り込みバーが開いたまま条件だけ消える**と推定される。
比較: `src/app/top_level_grid_view.rs:703-710` の `TopLevelGridSurface::Folder` アームは
ローカル検索を退避して `execute_search` で再適用する。Collection アーム (`743-745`) は
`open_collection_grid` を呼ぶだけ。
(Rating / Bookmark も再適用しないので「合成一覧としては同等」ではあるが、
バーが開いたまま空になる点は Collection だけの見え方。)

### A-12 (P3) 横断一覧なのに「場所」ファセットが強制表示されない
`src/ui_main.rs:10401-10408` は `should_force_place = self.items_are_rating_view || place_keys 非空` で、
レーティング一覧だけ「場所」ファセットを強制的に出す。
コレクション root は仕様上まさに複数フォルダ横断なのに対象外。
(ブックマーク / スマートフォルダも対象外なので既存の判断と一貫はしている。要判断。)

### A-13 (P3) エクスプローラからの D&D が「コピー先のフォルダがありません」で拒否される
`src/app.rs:32482-32497` `handle_external_file_drop` は `current_favorite_target()`
(`src/app.rs:19977-19992`、`current_folder` 必須) を宛先にするので、コレクション root では
`None` → トースト「コピー先のフォルダがありません」。
コレクションでの自然な期待は「参照として追加」なので、文言も動作も噛み合わない。
最低でも文面をこの surface 専用にする (例「コレクションへはツールバーの『追加』で登録してください」)。

### A-14 (P3) 管理 window / 並べ替え window の本棚とのパリティ差分
- 管理 window の行に件数が無い。本棚は `{n}p` を行とコンボ両方に出す
  (`src/ui_main.rs:7000`, `8529`)。コレクションは名前のみ
  (`src/ui_dialogs/collections.rs:2676-2678`, `src/ui_main.rs:1626-1636`)。
- 本棚の管理には「更新」ボタン (`src/ui_main.rs:6967-6975`) があるがコレクションには無い
  (actor watch で収束する設計なので妥当と思われる)。
- 並べ替え window: 本棚は「このページを <別の本> へ コピー / 移動」を持つ
  (`src/ui_main.rs:7515-7600`)。コレクションには「参照を別のコレクションへ移す」相当が無い
  (`src/ui_dialogs/collections.rs:3306-3380`)。仕様は要求していないので要判断。
- 選択件数ラベル: 本棚は 0 件時に「選択 0 ページ」、コレクションは 0 件でも
  「選択 0 件（ここをドラッグして移動）」とドラッグ案内が付く
  (`src/ui_main.rs:7487-7492` vs `src/ui_dialogs/collections.rs:3334-3339`)。

### A-15 (P3) コレクション root で BS / ⬆ が無反応
`src/app.rs:18973-19006` `grid_parent_nav_target()` は
`collection_grid_parent_nav()` (root では None: `src/app/collection_grid.rs:461-471` が
`PhysicalSource` のときだけ Some) → 各 `items_are_*` すべて false → `effective_folder()?` が None
で終了。したがって親ボタンは無効、BS も無反応。
比較: Bookmarks は `HistoryBack` (`src/app.rs:18981-18986`)、Rating は `RatingViewBack`
(`18987-18989`)、閲覧履歴も back nav を返す。SmartFolder root は同じく None
(`src/app.rs:18997-18999`) なので前例はある。**5 つの合成 surface のうち 3 つが「BS で抜ける」、
2 つが「無反応」**という状態なので、どちらへ揃えるか決めておきたい。

### A-16 (P3) `version_highlights::TABLE` に v4.0.0 の「重要な変更点」が無い
`src/version_highlights.rs:853` の最新は `"3.10.0"`。
コレクションは上部メニュー 1 本とツールバーセクション 1 本を**既定 ON** で増やす
(`src/settings.rs:4279-4281` `show_toolbar_collections` の既定 true、
`src/keymap.rs:2406` `TopMenuId::Collections`) ので、
リリース手順 Phase 1-5.5 の「操作・既定の変更」に該当する。出荷前に追記が要る。

### A-17 (P3) コレクションの各 modal が Esc で閉じない (本棚の削除確認は閉じる)
`src/ui_dialogs/collections.rs:2984`, `3018`, `3048`, `3099` の `egui::Modal::show(...)` は
戻り値の `should_close()` を使っていない。
比較: `src/ui_main.rs:7071-7101` (製本の削除確認) は `response.should_close()` を見て閉じる。
なお `collection_operation_modal_visible()` は `common_modal_dialog_open`
(`src/app.rs:18365`) に登録済みなので Esc は背面へ漏れない = 完全に「効かないキー」になる。
`CollectionDialogOperation::Name` は TextEdit を含むので、閉じるようにする場合は
CLAUDE.md「IME 対応」に従い `dialog_escape_pressed` 相当を通すこと
(現状は Esc を扱っていないので IME 破壊は起きていない)。

### A-18 (P3) Remote と PC の文言差 (参考、1 行記録)
`crates/remote-web/web/app.js:4892, 4898` の
「並べ替えは mIV 本体の**コレクション設定**で変更できます」は、PC 側に存在しない名称
(実体は上部「コレクション」メニュー →「現在のコレクションの並び順」)。
また空一覧の文言が Remote 「このコレクションには表示できる項目がありません。」(app.js:5109) と
PC 「コレクションに項目はありません」(`src/app/collection_grid.rs:1017`) で不統一。

---

## 追加監査項目 (コーディネータ指示)

### (1) フルスクリーンでの Delete — 結論: **どちらでもなく「何も起きない」。仕様どおりだが入口自体が無い**

- `KeyAction` にフルスクリーンの削除操作は **存在しない**。
  `src/keymap.rs` の Fs 系で delete と付くのは `FsDeleteEraseMask` (1750) /
  `FsDeleteConcealMask` (1751) の**マスク削除だけ**で、ファイル削除ではない。
- 唯一のグリッド Delete ハンドラ `src/ui_dialogs/context_menu.rs:2070` `handle_delete_key` は
  冒頭 `2076-2081` で `if self.viewer_session_blocks_main_window() || … { return; }`。
  `src/app.rs:41313-41322` によりこれは「埋め込みフルスクリーン表示中」に true。
  → **埋め込みフルスクリーン中の Delete は完全に no-op**。
- フルスクリーンの右クリックメニューにも削除が無い。
  `src/context_menu_model.rs:1441-1443` の `MoveToRecycleBin` も
  `1427-1429` の `RemoveFromCollection` も `input.surface == ContextMenuSurface::Grid` を要求する。
  `request_delete_confirm` の呼び出し元も Grid 経路 2 箇所のみ
  (`src/ui_dialogs/context_menu.rs:1756`, `2114`, `2128`)。
- したがって「元ファイルをゴミ箱へ移す」ことも「参照を外す」ことも**起きない**。
  仕様の境界 (root 直下 = 参照解除 / 物理 child = 元ファイル) に違反はしない。
- なお **detached (F12) で別窓に出している間はメイン一覧が生きる**ため、
  `viewer_session_blocks_main_window()` は false になり
  (`src/app.rs:41319-41321` のコメントが明示)、メイン一覧がコレクション root なら
  Delete は `collection_root_delete_resolution` 経由で**参照解除**になる
  (`src/ui_dialogs/context_menu.rs:2093-2104`)。これは仕様どおり。
- **評価**: 欠陥ではない (通常フォルダのフルスクリーンでも削除手段は無い) が、
  A-4 と同じ「読みながら操作できない」制約の一部。プレイリスト用途で
  「今見ているこれを外す」をフルスクリーンから行いたくなる可能性は高い。**P3 相当の将来要望**として記録。

### (2) 上部「表示」メニューの「ソート順」 — 結論: **ツールバーと同じ空振り (A-1 に含む)**

`src/ui_main.rs:6493-6528`。
- 無効化: `sort_lock` のみ → A-1 と同じ理由で Collection では無効にならない。
- 表示値: `let checked = if self.items_are_rating_view { rating_view_sort == Normal(order) }
  else { self.settings.sort_order == order }` (`6507-6512`) →
  Collection では `settings.sort_order` の項目に ✓ が付く。実際の並び (手動順 / collection の
  `standard_sort`) とは無関係。
- クリック: `self.settings.sort_order = order;` の後、rating 以外は `sort_changed = true`
  (`6517-6526`) → `apply_sort_change_reload` が no-op (`src/app.rs:20579`)。
  → **ツールバーと完全に同じ挙動**。ただしメニュー側は `settings.save()` を呼んでいない
  (`6518` は代入のみ) ので、永続化のタイミングだけツールバーと違う (`src/ui_main.rs:8863`)。
- **ついでの既存不整合 (Bookmark 一覧)**: この表示メニューは `items_are_bookmark_view` を
  見ていないため、(a) ブックマーク一覧で `bookmark_view_sort` が `CreatedAtDesc/Asc` のとき
  `settings.sort_order` 側の項目に誤って ✓ が付く、(b) ツールバーにはある
  「登録日時↓ / ↑」(`src/ui_main.rs:8891-8905`) がメニューには無い
  (レーティングの `RatedAt` は `6529-6543` にある)。Collection 対応とは独立の既存差分。

### (3) 「元の場所を開く」 — 結論: **未実装。`OpenFolderInExplorer` は代替にならない** → A-6 に記載

`src/context_menu_model.rs` に存在するのは
`JumpToFolder`「フォルダに移動」(64, 175, 1350) と `OpenFolderInExplorer`
「このフォルダをエクスプローラで開く」(1412-1419) の 2 つ。前者は `view.in_search` 限定で
コレクションでは出ず、後者は外部 Explorer を起動するだけで mIV 内の一覧は動かない。
「登録元フォルダを mIV 内で開く」「そのファイルを選択状態で開く」操作は **存在しない**。
詳細と修正方向は A-6。

---

## D-1 (設計提案) 「手動順」をソート選択の一級市民にする

A-1 の修正には 3 案ある。**案 C を推す。**

**案 A: 手動順を `PageOrderFixed` でロックする (最小)**
`page_order_locked_for_current_view()` に「Collection surface かつ `order_mode == Manual`」を足す。
- 利点: 既存の「固定」表示 + ツールチップがそのまま使え、A-2 の列ヘッダソートも同時に塞がる。
- 欠点: 通常ソート時の表示値が `settings.sort_order` のままズレ続ける (`standard_sort` と別物)。
  手動順⇄通常ソートの切替が上部メニューにしか無い状態も変わらない (利用者の希望と逆)。

**案 B: `GridSortLockReason` に `CollectionOrder` を足し、短縮表示を「手動順」/ 実ソート名にする**
- 利点: 表示のズレは消える。実装は小さい。
- 欠点: 「ソートのプルダウンから手動順を選びたい」という利用者の希望を満たさない。

**案 C (推奨): `RatingViewSort` / `BookmarkViewSort` と同じ「ビュー専用ソート型」へ揃える**
既に先例がある: `rating_view::RatingViewSort::Normal(SortOrder)` + 時刻順 2 種を
同じコンボへ並べ、`apply_sort_change_reload` の rating 分岐が `Normal(settings.sort_order)` へ
同期する (`src/app.rs:20526-20536`, `src/ui_main.rs:8876-8890`)。
コレクションは既に `CollectionOrderMode { Manual, Standard } + standard_sort: SortOrder` という
**同じ形の型を DB 側に持っている** (`src/collection_store/model.rs:186`)。UI はそれを読むだけでよい。

具体的には:
1. **表示値**: Collection root では `settings.sort_order` を見ず、
   `collection_definition(id)` から `order_mode == Manual ? "手動順" : standard_sort.short_label()`
   を出す。ツールバーの Dropdown の `current_text` (`src/ui_main.rs:8907-8915`) と
   Buttons の `selected` (`8838-8850`) の両方。
2. **選択肢**: 既存のソートボタン列の先頭 (または `ui.separator()` の後) に
   **「手動順」ボタンを 1 個追加**する。`items_are_rating_view` が `RatedAtDesc/Asc` を
   足しているのと同じ場所・同じ書き方 (`src/ui_main.rs:8876-8890`, `8961-8979`,
   メニューは `6529-6543`)。
3. **`settings.sort_order` を汚さない**: Collection 分岐では
   `self.settings.sort_order = order;` を**実行しない**。
   `start_collection_grid_content_action(target, SetOrder { mode: Standard, sort: order })`
   (`src/ui_main.rs:6193-6199` と同じ呼び出し) だけを発行し、actor の revision 更新で
   一覧が収束するのを待つ。手動順ボタンは `SetOrder { mode: Manual, sort: 現在値 }`。
   — これは rating / bookmark が `settings.sort_order` を書いてから同期しているのとは
   あえて変える。コレクションの並びは**永続 DB の属性**であり、
   `docs/collection-implementation-plan.md` §2.4 が
   「collection の標準ソートは通常一覧の `SortOrder` 値を明示保存するが、他設定を更新しない」と
   明記しているため。
4. **`apply_sort_change_reload`**: Collection 分岐を足して**早期 return** する
   (actor 経由で更新されるので UI 側の再ロードは不要)。
   `src/app.rs:20511` の分岐列の先頭付近、`items_are_rating_view` と同じ並びに置く。
5. **詳細表示の列ヘッダとの関係**: 手動順のときは
   `page_order_locked_for_current_view()` を true にして
   `GridSortLockReason::PageOrderFixed` を返す (案 A と併用)。
   これで列ヘッダソートが `sort_enabled = false` (`src/ui_main.rs:15638`) になり A-2 も閉じる。
   通常ソートのときは列ヘッダソートを許すが、`open_collection_grid` で
   `reset_details_sort_to_toolbar()` を呼び、**別フォルダから持ち込まれた列ソートは効かせない**
   (§1.143(a) の規約どおり)。
6. **上部メニューの「並び順」は残す**。手動順⇄通常ソートの切替が 2 箇所になるが、
   同じ 1 つの述語 (`collection_definition().order_mode`) から表示を導けば食い違わない。
   `src/app.rs:51406-51410` のコメントが求める「ツールバーもメニューも同じ 1 つの述語を見る」を
   Collection でも成立させることが要件。
7. **ヘッダ表示**: 仕様「ヘッダにコレクション名と並びを表示する」を満たすため、
   `src/app/collection_grid.rs:1417` の `address` を
   `コレクション: {name}（手動順）` / `（{sort名}）` にする案もあるが、
   1.〜2. でツールバーに並びが出るなら重複なので任意。

---

## 監査したが問題なしと判断した項目 (再監査防止)

- **`items_are_*` 分岐の網羅**: `src/` 配下 (テスト除く) の `items_are_*` 参照を機能別に確認した。
  Collection が「物理フォルダ扱い」に落ちても**安全側**に倒れる経路は以下。すべて
  `current_folder == None` が偶然のガードになっている点だけ注意 (将来 `current_folder` に
  合成パスを入れると一斉に壊れる)。
  - 代表サムネ pin: `src/app.rs:29039` (`current_folder.as_ref()?`) と
    `src/ui_dialogs/context_menu.rs:1441` → ボタンもメニュー項目も出ない。正しい。
  - ファイル名スタック: `src/filename_stack_ui.rs:453` (`current_folder.is_some()`) → 無効。正しい。
  - サブフォルダ展開: `src/app/subfolder_expansion.rs:1445` → 無効。正しい。
  - お気に入り追加: `src/app.rs:19977-19992` → 対象なし。正しい。
  - メタデータ転送: `src/ui_dialogs/metadata_transfer.rs:246` (`current_folder_last_mtime`) → 無効。
    ただし Collection は**明示列挙されていない**ので、ガードが暗黙。
  - 外部ツールのコンテナ対象: `src/external_tool.rs:2373-2387` → `effective_folder()` が None で空。
  - 見開き設定キー: `src/app.rs:20629-20644` → None。
  - 空一覧での背景右クリック: `src/ui_dialogs/context_menu.rs:864-871` が
    `current_folder` None で `return None` → メニューが出ない。フォルダ操作が無意味な面なので妥当。
- **F5 (一覧更新)**: `src/app/top_level_grid_view.rs:743-745` に Collection アームがあり
  `open_collection_grid` へ振り分けられている。`reload_top_level_grid` の網羅 match も維持。
- **Delete キー / コンテキストメニューの参照解除 (正常系)**:
  `src/ui_dialogs/context_menu.rs:2093-2104` は checked 優先 / selected fallback で
  `Ready` のときだけ Remove、`Unavailable` は toast で fail closed。
  `CollectionPlaceholder` は `supports_delete()` が false (`src/context_menu_model.rs:819-821`)
  なので元ファイル削除が出ず、`RemoveFromCollection` だけが出る。仕様どおり。
  (異常系は A-3。)
- **Windows シェルメニューの submenu 化**: `src/ui_dialogs/context_menu.rs:414-424`
  `native_grid_shell_menu_placement` が Collection root を `CollectionSourceSubmenu` に固定。
  グローバルの Inline 設定に引きずられない。§20 の記述どおり。
- **ゴミ箱メニューのラベル分離**: `src/context_menu_model.rs:1207-1219` (checked) /
  `1441-1455` (単一) が `collection_source_context` で「元ファイルを…」に切り替える。妥当。
- **`MenuCommandId::CollectionsRelinkCurrent` の残骸**: `menu_command_is_available_in_build`
  (`src/keymap.rs:2960-2967`) が false を返し、`menu_commands_for_parent` (`2969-2975`) が
  除外する。操作カスタマイズ UI も `menu_commands_for_parent` 経由
  (`src/ui_dialogs/preferences/pages.rs:5068, 5148`) なので**幽霊項目は出ない**。
  保存済み ID の読取互換だけ残す扱いは妥当。`src/ui_main.rs:6207-6210` の空アームも意図的。
- **ツールバー Collections セクションの本棚同型性**: コンボ(全件) 常設 / 追加 / 開く /
  ▶▼ 折りたたみ / 固定ボタンの左=開く・右=追加 / 右クリックのセクション設定
  (`src/ui_main.rs:1600-1704`, `8507-8630`, `9835-9848`)。
  表示形式は本棚と同じ `TD::all_collapsible_only()` の 2 択。
  無効時のツールチップも 3 状態 (`1646-1652`)。妥当。
  - 軽微: `draw_collection_toolbar_controls` の `has_add_target` に
    本棚用の変数名 `has_book_add_target` (`src/ui_main.rs:8272`) をそのまま渡している
    (`8622`)。中身は `selected.is_some() || !checked.is_empty()` で汎用なので**動作は正しい**。
    命名だけ紛らわしい。
- **管理 window の規約**: `.open(&mut open)` 付き (`src/ui_dialogs/collections.rs:2579`)、
  `ScrollArea` は `auto_shrink([false,false])` (`2645`, `3437`)、
  TextEdit は全て `crate::ime_focus::add_singleline` 経由 (`2618`, `2676`, `2990` — raw TextEdit なし)、
  `common_modal_dialog_open` へ 3 つとも登録済み (`src/app.rs:18365-18367`、しかも
  state の有無ではなく `*_visible()` / `*_open()` の述語から導出している)。
  配色も `ui.visuals().warn_fg_color` / `error_fg_color` / `weak` を使い固定 gray なし。
  dark / light スナップショットも `tests/snapshots/` に 5 枚ある。妥当。
- **削除 / 参照解除の確認文言**: 「定義と参照を削除します」「元のファイルやフォルダは削除しません」
  (`src/ui_dialogs/collections.rs:3018-3030`)、
  「N 件の参照をコレクションから外します」「元のファイルやフォルダは削除しません」(`3048-3062`)。
  内部用語 (actor / revision / snapshot / stale) の露出なし。妥当。
- **専用並べ替え window の操作パリティ**: 単一 / Ctrl / Shift 選択、グループドラッグ、
  挿入マーカー、左右移動、サムネサイズスライダ、Home/End/PageUp/PageDown
  (`src/ui_dialogs/collections.rs:3306-3420`)。Conflict 時は「最新内容を読み直す（変更破棄）」
  「変更を破棄して閉じる」で明示回復 (`3290-3304`)。Standard 中は入口が
  disabled + 理由付き (`src/ui_main.rs:6211-6225`)。仕様どおり。
- **missing 項目の表示**: `GridItem::CollectionPlaceholder` を専用色 + `?` + ファイル名 +
  理由ラベルで描画 (`src/app/grid_paint.rs:706-740`)。理由は
  「見つかりません / 未対応の形式 / 読み取れません」(`src/grid_item.rs:132-140`)。
  サムネイル状態は `Failed` 初期化で無駄な repaint を誘発しない (`src/app.rs:27694-27696`)。
  仕様「見つかりません」と一致。
- **UI 文言のグリフ**: `python scripts/check_ui_glyphs.py` → 0 件 / exit 0。
  新規文字列に危険グリフ無し (`●` `▶` `▼` `←` `→` `?` `✓` は既存採用済みの範囲)。
- **内部用語の露出**: トースト / ツールチップを通読した限り actor / revision / snapshot / stale /
  worker などの露出なし。「コレクション一覧を更新中のため、登録解除できません」等、
  利用者語で書かれている (`src/app/collection_grid.rs:379, 387, 394, 400, 406, 414`)。
- **空一覧・読み込み中の案内**: `collection_grid_empty_message()`
  (`src/app/collection_grid.rs:1007-1029`) が Root のときだけ
  「コレクションを読み込み中…」「コレクションに項目はありません」「コレクションは削除されました」
  を返し、`src/ui_main.rs:16313-16341` で最優先に表示される。
  一般の「フォルダを入力して Enter キーを押してください」に落ちない。妥当。
- **エラー overlay**: `render_collection_grid_error_overlay` (`src/ui_main.rs:8192-8217`) が
  installed 済みで Failed のときだけ出す。旧一覧を消さない。妥当。
- **アドレスバー表記**: `コレクション: {name}` (`src/app/collection_grid.rs:1417`) は
  「閲覧履歴」「ブックマーク」「タグビュー: …」「★★ レーティング一覧」と同じ慣習
  (`src/app.rs:23994, 33822, 23560, 24059`)。妥当。
- **履歴 (戻る / 進む)**: `FolderNavHistoryTarget::Collection` が typed で入り、
  明示 Open だけが履歴を作る (`src/app.rs:19535-19551`、`docs/top-level-grid-view.md` §3)。
  合成パスへ射影していない。妥当。
- **ナビゲーション / 再生の入口**: Ctrl+↑↓ (grid / fullscreen)、スライドショー、
  動画 / 音声モード / 音楽の EOF がすべて collection 専用 producer を持つ
  (`src/app/collection_navigation.rs:877, 887, 986, 1024, 1063, 1113, 1137, 1160`)。
  UI 入口の欠落は見当たらない (中身の正しさは別担当の範囲)。
- **操作カスタマイズのプレビュー**: `ContextMenuPreviewScenario::GridCollectionItem`
  「一覧：コレクションの項目」が追加済み (`src/context_menu_model.rs:391, 406, 423, 455`)。妥当。
- **カラー検索 / ★フィルタ / ファセット / チェック操作**: いずれも `items` 汎用処理で
  コレクション root でも動く。`color_filter_available_in_current_view`
  (`src/app/color_filter.rs:30-38`) は Collection を除外していないが、ブックマーク /
  スマートフォルダも除外していないので既存判断と一貫。スコープ署名は items のハッシュ
  (`src/color_search.rs:190`) なので別コレクション同士で混ざらない。
