# タグビューの本の移動をタグの結果の順にする (§1.802)

2026-11-18 第 2 版 (ClaudeCode、第 1 版の独立レビュー REVISE を反映、§9)。設計・作業指示・検収 = ClaudeCode、実装 = Codex gpt-6.1-sol / high、
独立レビュー = 別の gpt-6.1-sol / xhigh。複数の入力の入口・全画面・別窓にまたがるので、実装前に独立レビューを受ける。
コードの事実は master 605ba335b を読んだもの。実行時の観測は無い。

- 2026-11-18 利用者決定 (統括経由、AskUserQuestion): **案 B「タグの結果の順に移る」**。一覧でも全画面でも、
  タグの結果の中で前後の本・フォルダへ移る。案 A (全画面も移動しない) は採らない。
- この決定で、出荷済みの全画面の動き「タグビューの本から Ctrl+PageUp/Down・Ctrl+↑↓ で物理フォルダの兄弟・前後へ抜ける」は無くなる。
  更新履歴に変わった点として書く (§8)。

## 1. 現状 (コード)

- タグビューの状態は `TagViewState` (`src/tag_view.rs`)。結果のパスは保持しない (`result_count` だけ)。
  結果の一覧は通常の `items` で、結果から本・フォルダを開くと `items` は本の中身に置き換わる。
  `tag_view.nav_stack` に開いた順のパスが積まれ (`record_tag_view_nav_open`、`commit_staged_search_navigation_effects` から)、
  BS は `tag_view_back` で 1 段戻り、空になると `execute_tag_view` で**検索し直して**結果一覧へ戻る。
- 本を開いた後も、最上位の surface は `TopLevelGridSurface::Search(TopLevelSearchView::Tag)` のまま
  (`items_are_tag_view` だけ false に戻る。`src/app/top_level_grid_view.rs` 約 1536)。
- 一覧 (`src/app.rs` の `handle_grid_navigation_keys` 相当、約 60185〜): `in_tag_view = self.tag_view.active` のとき
  Ctrl+↑↓ は `cancel_pending_folder_nav()` だけ、Ctrl+PageUp/Down は何もしない。
- 全画面:
  - `handle_fullscreen_ctrl_nav_context` (`src/ui_fullscreen.rs`) には `tag_view.active` の判定が無く、最後の
    `start_folder_nav(effective_folder, FolderNavMode::Fullscreen)` (物理の木の DFS) に落ちる。
  - `handle_fullscreen_sibling_nav_context` も同様に `SiblingFullscreen` に落ちる。
  - ロック中の継続 `locked_fullscreen_folder_nav_request` にも判定が無い。
  - スライドショーの「次のフォルダ」(`slideshow_advance_folder` 相当、約 42860) だけは、`tag_view.active` で false を返す (ループに任せる)。
- 一覧の順の操作 (§1.444、[一覧の表示順で前後の本へ移る操作](list-order-book-navigation-plan.md)) の仕組み:
  - `ListOrderSnapshot { owner, scope, entries }` を `ViewerContextBundle` の 1 field に持つ。
  - 本の行を開く操作を受け付けた時点で、`capture_main_folder_list_order(idx)` が `current_grid_order()` の本の行
    (Folder / Zip / Pdf / ConvertibleArchive) を凍結して写す。
  - 位置は持たず、`list_order_position()` が表示中の本から毎回求める。
  - 移動は `FolderNavMode::ListOrder { snapshot, from_index }` (全画面専用) の worker が、Ctrl+↑↓ と同じ停止条件で行き先を決める。
  - scope は `Folder` と `Everything { run }`。タグ検索は §1.444 の §6 で範囲外にしていた。
  - Everything では、一覧の順の移動の着地だけが `EverythingChildAdmission::ListOrderRow` で検索側の帰路の基準 (`logical_source`) を移動先の行へ置き換える (§1.682)。

## 2. 方針: 一覧の順の仕組みに `Tag` scope を足し、タグビューの本では Ctrl+↑↓・Ctrl+PageUp/Down をそれへ向ける

新しい移動の仕組みは作らない。凍結した列・毎回求める位置・worker の停止条件・既存の gate・着地は、§1.444 と §1.682 のものを使う。
足すのは次の 4 点だけ。

1. scope `ListOrderScope::Tag { run }` と、その列の取得・位置の求め方 (§3)
2. 一覧 (Grid) で着地する ListOrder (§4.1)
3. タグビューのときの入口の振り分け (§4.2)
4. 着地で `tag_view.nav_stack` を移動先の行 1 つに置き換える (§4.3)

## 3. 列と位置

### 3.1 実行の番号

- `TagViewState` に `run: u64` を足す。session 内だけで持ち、保存しない。
- 結果一覧を出す実行 (`execute_tag_view`) を受け付けるたびに、単調に増やす。BS で結果一覧へ戻るときの再検索も、
  新しい番号になる。その後に結果から本を開けば、新しい列に置き換わる。
- `dismiss_tag_view_with_origin` は番号を戻さない (単調のまま)。古い列は scope 不一致で使えなくなる。

### 3.2 `current_list_order_scope()`

- surface が `Search(Tag)` で `tag_view.active` のとき `Some(ListOrderScope::Tag { run: tag_view.run })`。
- main context でない、または ★固定が有効なら、従来どおり `None`。

### 3.3 取得 (`capture_main_folder_list_order`)

- `Tag` scope では、**結果一覧そのもの** (`items_are_tag_view` かつ `tag_view.nav_stack` が空) から本の行を開く操作を受け付けたときだけ取る。
- 結果から入ったフォルダの中 (nav_stack が空でない) から開いたときは取らず、既存の列を残す
  (Everything の `PhysicalChild` と同じ扱い)。
- 取るもの・入口 (Enter・ダブルクリック・ページ表示/ページ一覧で開く・ゲームパッドの決定・変換対象の書庫) は既存のまま。
  並べ替え (`TagViewSort`)・種類の絞り込み・ファセットは `current_grid_order()` が既に反映している。
- 画像・動画・音声の行は列に入らない (既存の `ListOrderEntry::from_item`)。

### 3.4 位置 (`list_order_position`)

`Tag { run }` では、次をすべて満たすときに位置を返す。

1. §1.444 §3.4 の 1〜4 (列がある・owner が main・表示先が MainWindow か Fullscreen・scope が一致) を満たす。
2. `tag_view.nav_stack.first()` (結果から開いた行。以下「根の行」) がある。結果一覧の上と、結果の画像の全画面では、根の行が無いので位置も無い。
3. 列の中に、根の行と `path_eq` で一致する行がある。
4. 表示中の本 (`effective_folder()`) が根の行と同じか、根の行が Folder のとき、その配下である (成分境界。既存の `path_in_subtree_ci`)。

- 根の行で決める理由: タグの結果には、タグを付けたフォルダとその中のタグを付けたフォルダが両方入り得る。
  表示中のパスの祖先で探すと、どちらの行か一意に決まらない。nav_stack の先頭は利用者が開いた行そのものなので、これを使う。
- 根を保つ (r1 P2-4): 今の `record_tag_view_nav_open` は、上限 `MAX_FOLDER_NAV_STACK` (100) を超えると先頭を消す。
  これを、**先頭 (根の行) を残して 2 番目を消す**形に変える。
  - 101 段の降下の後も根の行が残る。間引いた後の先頭が、入れ子の別の結果行にすり替わることも無い。
  - 位置のための新しい field は足さない。
  - BS で戻る途中の段が間引かれるのは、今と同じ性質。根の行からは結果一覧へ戻れる。
- 4 を満たさない (タグビューのまま、ほかの経路で結果の外へ移った) 場合は、位置は無い。

## 4. 移動

### 4.1 一覧で着地する ListOrder

- `FolderNavMode::ListOrder` に `fullscreen: bool` を足す (`Favsearch` / `SmartFolder` と同じ形)。
- 既存の `FsListOrderPrev/Next` は `fullscreen: true` で、挙動を変えない。
- `fullscreen: false` の着地は、同じ行き先へ `FolderNavMode::Grid` で着いたときと同じにする。
  ZIP / PDF の後からの採用・変換書庫・`pre_scan` の扱いも同じ。
- worker の行き先の決め方は共通で、新しい判定は作らない。
- `FolderNavMode` の match をすべてたどって揃える:
  - `folder_nav_mode_same_kind`・`StartupListIntent::folder_navigation`・`history_trigger`・`perf_tag`
  - 端の表示 (一覧は Grid の DFS の端と同じ通知、全画面は既存の `FsBoundaryHint::Edge`)
  - `inflight_fullscreen_folder_nav_mode` (ListOrder は継続対象にしない。§1.444 §4.2 のまま)
- 一覧の受付の gate (r1 P2-2): 全画面の sequence / 表示待ち lock は一覧には効かないので、一覧用に次の契約を置く。
  - **一覧の ListOrder は、main の一覧の移動が採用されるまで次の要求を拒否する**。拒否する期間は次の 2 つ:
    - `folder_nav_pending` がある間
    - worker の回収後、行き先の採用を待つ間。ZIP / PDF / 変換書庫の `PhysicalHistoryTransition`・staged open・hydration のうち、
      folder navigation が持ち主の要求が対象。
  - 拒否は、既存の要求を cancel・再開始せずに return する。拒否はログに残す (全画面の `note_fullscreen_nav_request_dropped` と同じ扱い)。
  - 「folder navigation が持ち主の要求が採用待ちか」の述語は、既存の要求の持ち主の情報 (`MainListRequestOwner` など) から 1 つ作る。
    新しい bool や pending は足さない。既存の情報で表せないなら、実装前に止めて報告する。
    - r2 の補足: `MainListRequestOwner` は保存されない受付引数なので使えない。採用待ちに残る移動モード
      (`PhysicalHistoryTransition.dfs_continuation` と `MainListAfterHydration::Dfs`) から判定する。
  - §1.805 の DirectNavigation の走査待ちは拒否の理由にしない。ListOrder の受理 (`start_folder_nav`) の既存の受付点で退役させる (§1.805 の裁定のまま)。
  - 累積 (`MAX_PENDING_NAV`) には乗せない (§1.444 のまま)。
  - 既存の Grid の Ctrl+↑↓ (物理の DFS) の受付は変えない。

### 4.2 入口の振り分け

タグビュー (`tag_view.active`) のときの動きを、次の表にする。表の「一覧の順」は、§3.4 の位置があれば ListOrder を始めることを指す。

| 入口 | 根の行がある (本の中) | 根の行が無い (結果一覧の上・結果の画像の全画面) |
| --- | --- | --- |
| 一覧 Ctrl+↑↓ | 一覧の順 (`fullscreen: false`) | 今と同じ (何もしない) |
| 一覧 Ctrl+PageUp/Down | 一覧の順 (`fullscreen: false`) | 今と同じ (何もしない) |
| 全画面 Ctrl+↑↓ | 一覧の順 (`fullscreen: true`) | 案内「検索結果を開いてからCtrl+↑↓で移動できます」(既存の `SearchResultList`) |
| 全画面 Ctrl+PageUp/Down | 一覧の順 (`fullscreen: true`) | 案内「検索中は兄弟フォルダ移動しません」(既存の `SearchSiblingUnsupported`) |
| 全画面 Ctrl+Shift+↑↓ (FsListOrder) | 一覧の順 (既存の操作のまま) | 既存の「一覧から開いた本で使えます」 |
| スライドショーの次のフォルダ | 今と同じ (ループに任せる) | 今と同じ |

- 位置が無い (§3.4 の 1・3・4 を満たさない) ときは、何もせず案内だけを出す。物理の木の DFS・兄弟移動へ黙って切り替えない。
  一覧は「一覧から開いた本で使えます」、全画面は同じ文言の案内とする。
- **タグビューの判定の置き場所**: 一覧・全画面とも、物理の DFS・兄弟移動へ落ちる前に置く。
  - ★固定 (snapshot) と Ctrl+G・お気に入り検索 (favsearch) の分岐は、今の優先順のまま。タグビューと同時に有効にはならない前提を、実装時にコードで確かめる。
  - 独立した別窓 (`detached_physical_folder_nav_available`、`independent_collection_sibling_navigation_available`) は、今どおりタグビューより先に判定する。
    これらは main のタグビューの状態を見ない設計なので、変えない。
  - 全画面のロック中の継続 (`locked_fullscreen_folder_nav_request`) は、タグビューでは `None` を返す
    (物理の `SiblingFullscreen` / `Fullscreen` の継続を返さない)。進行中の ListOrder は、§1.444 どおり継続しない。
- Ctrl+↑↓ と Ctrl+PageUp/Down は、タグビューの本では同じ動きになる。結果は平らな列で、配下の木を降りる操作が無いため。
- 入口の列挙 (r1 P2-1 で訂正)。
  - **全画面**: キーボード、ROOT が受けた全画面キー、native 動画の presenter、ゲームパッドは、共通 handler
    (`handle_fullscreen_ctrl_nav_context` / `handle_fullscreen_sibling_nav_context`) を通る。
  - **ページ一覧の受付** (`page_overview_folder_navigation_available`、`src/ui_fullscreen/page_overview.rs`) は別の述語。
    タグビューでは、受け付けるかどうかを handler と同じ判定 (§3.4 の位置があるか) にそろえる。
    位置が無いのに受け付けると、移動できずにページ一覧だけが閉じる。
  - **一覧**: 入口が 5 つに分かれていて、タグビューの扱いも揃っていない。
    1. 通常のキー処理 (`src/app.rs` 約 60185〜): Ctrl+↑↓ と Ctrl+PageUp/Down を処理する。タグビューでは止まる。
    2. 項目入力停止中のキー処理 (`src/app.rs` 約 59376 の `GridTreeFolderPrev/Next`・`GridSiblingFolderPrev/Next` のループ): タグビューの判定が無く、物理の移動を始める。
    3. リング・ジェスチャの木の移動 (`handle_grid_tree_folder_nav`、`src/app/gamepad_input.rs` 約 8263): タグビューで止まる。
    4. リング・ジェスチャの兄弟移動 (`apply_sibling_folder_nav` の一覧側): タグビューで止まる。
    5. ゲームパッドの肩ボタン (`handle_gamepad_grid_folder_nav`、約 9915): タグビューの判定が無く、物理の DFS・ZIP 内 DFS へ進む。
  - 一覧用の helper を 1 つ作り、5 つの入口のすべてから同じ位置で呼ぶ。位置は、★固定・Ctrl+G・Ctrl+F・お気に入り検索の分岐の後、
    物理の移動 (ZIP 内 DFS・一覧の由来の移動・`start_folder_nav`) の前。
    - helper の名前の例: `route_tag_view_grid_book_navigation(ctx, forward) -> bool`。タグビューなら処理済みとして true を返す。
    - 根の行がある: §4.1 の gate を通して ListOrder (`fullscreen: false`)。
    - 根の行が無い (結果一覧の上): 今と同じく、進行中の folder nav を取り消して何もしない。
  - 5 つの入口のほかの分岐 (★固定・検索など) を 1 つの router にまとめ直すことは、今回はしない。
    今回の範囲はタグビューの判定の追加に限る。
  - 2 と 5 は、タグビューの本で今は結果の外へ抜けている。この変更で抜けなくなる (更新履歴の文案に含む)。

### 4.3 着地と nav_stack

- 着地 (採用が成功した時点) で、`tag_view.nav_stack` を `[移動先の行]` に置き換え、アドレス欄 (`update_tag_view_address`) を更新する。
  `favsearch_navigate_sibling` が `favsearch.nav_stack = vec![next]` にするのと同じ形。
- 今の `commit_staged_search_navigation_effects` は、`tag_view_origin: bool` で `record_tag_view_nav_open` (push) するだけ。
  これを、Everything の `EverythingChildAdmission` と同じく型で区別する:
  - `TagViewNavAdmission::{Descend, ListOrderRow { run, row }}` (名前は実装に任せる)
  - ListOrder の Tag scope の結果を採用するときだけ `ListOrderRow` を載せる (`apply_folder_nav_result` の Everything の分岐の隣)。
  - commit 時に `run` が今の `tag_view.run` と一致し、採用したパスが `row` と一致するときだけ置き換える。一致しなければ従来の push。
  - 運ぶ経路は、今 `tag_view_origin` が通る経路 (通常フォルダの install と `PhysicalHistoryTransition` の ZIP / PDF / 変換書庫) と同じ。
- Folder 行から中の書庫へ着く場合 (§1.444 §3.4 の例) は、Folder 行の採用で `[行]` にし、その後の書庫の open は従来の push (Descend)。
- 採用前の取消・失敗では nav_stack を変えない。
- 着地後の BS: 移動先の行へ直接着地したときは、nav_stack が `[行]` なので 1 回で空になる。既存の `tag_view_back` が結果一覧を再検索して戻る。
  - Folder 行から中の書庫へ着地した場合 (r1 P3-5): nav_stack は `[Folder 行, 書庫]` になる。最初の BS で Folder 行の一覧へ、次の BS で結果一覧へ戻る。
    結果から同じ Folder 行を開いて書庫へ着いた場合と同じ戻り方で、変えない。
- 結果一覧へ戻ったときの選択 (r1 P2-3): 今の `tag_view_back` は抜けてきたパスの**名前**を `select_after_load` に入れる。
  `try_select_after_load` は同じ名前の最初の項目を選ぶ。タグの結果は複数の親フォルダの項目を含むので、`C:\A\book.zip` と `D:\B\book.zip` が
  両方あると、後者から戻っても前者が選ばれ得る。これは今の結果一覧への BS でも起きる既存の問題。
  - 直し方: nav_stack が空になって結果一覧へ戻るときだけ、抜けてきた行を**完全なパス**で選ぶ。
  - 既存のパスによる選択の経路 (親へ戻るときの `child_selection` など) があればそれを使う。
    無ければ、名前の `select_after_load` と排他にした、パスを持つ選択要求を 1 つ足す (2 つを同時に持たない)。
  - 結果一覧の中で 1 段戻る場合 (nav_stack が残る場合) は、物理フォルダの中なので、今の名前による選択のままにする。

## 5. 状態遷移

| 出来事 | 列 | nav_stack | 位置 |
| --- | --- | --- | --- |
| タグの結果一覧から本を開く操作を受け付けた | Tag{run} の新しい列に置き換え | 採用時に push (今のまま) | 根の行で求まる |
| 本の中でサブフォルダ・中の書庫を開いた | 変えない | push (今のまま) | 根の行の配下なので求まる |
| タグビューの本で一覧の順に移動した (一覧・全画面) | 変えない | 採用時に `[移動先]` | 移動先の行 |
| BS で結果一覧へ戻った (再検索) | 変えない | 空 | 根の行が無いので無い。番号が変わるので、旧列は以後使えない。抜けてきた行を完全なパスで選ぶ |
| 降下が上限 (100) を超えた | 変えない | 根を残して 2 番目を消す | 根の行で求まる |
| 一覧の ListOrder の採用待ちに、もう一度押した | 変えない | 変えない | 拒否 (ログ) |
| 結果一覧の並べ替え・種類の変更・再検索 | 変えない | 空 | 番号が変わり、旧列は使えない。次に本を開けば置き換わる |
| タグビューを閉じた・別の surface へ移った | 変えない | 空 | scope 不一致で使えない |
| ★固定・F12 linked・park / mount・fork・promote | §1.444 §5 のまま | 今のまま | §1.444 §5 のまま |

## 6. 範囲外と理由

- **スライドショーの次のフォルダ**: 今はタグビューでループに任せている。結果の順に進める変更は今回の決定の範囲外 (利用者の決定は手動の移動)。
- **タグの結果一覧の上での Ctrl+↑↓**: 今と同じく何もしない。一覧に居るときは見えている順に選んで開けばよい (§1.444 §6 と同じ理由)。
- **結果の画像の全画面での本の移動**: 根の行が無いので案内だけにする。
- **別窓 (detached・F12 linked) でのタグの結果の順**: §1.444 と同じく使わない。独立した別窓は今どおり物理の移動。

## 7. 検討した単純化 (CLAUDE.md「設計の簡素化」)

- **既存の一覧の順の仕組みを使う**: 凍結した列・毎回求める位置・worker・gate・着地を共有し、タグ専用の移動を作らない。採用。
  - 作らなかった案: `favsearch_navigate_sibling` と同じ、結果のパスを持って `load_folder` で直接開く形。UI スレッドで同期走査になる。
    また、空のフォルダを飛ばす基準も別に要る。
- **位置を根の行で決める**: 祖先の探索の曖昧さを無くす。nav_stack は既存の状態なので、新しい状態は番号 1 つだけ。採用。
- **Ctrl+↑↓ と Ctrl+PageUp/Down を同じにする**: 平らな列に 2 種類の意味を作らない。採用。
- **準備中は受け付けない・累積しない**: §1.444 の gate のまま。乗り換えの状態を作らない。採用。
- **案内だけで、物理の移動へ切り替えない**: 位置が無いときに DFS へ落とすと、結果の外へ抜ける今の問題が残る。採用。
- まれな失敗は既定の割り切り (ログ + 通知)。

## 8. テスト・文書

### 8.1 テスト (handler-level と状態遷移)

- タグの結果から本 A を開くと Tag{run} の列が取れ、結果から入ったフォルダの中から開いても置き換わらない。
- 一覧・全画面で、Ctrl+↓・Ctrl+PageDown が結果の次の本 (停止条件で空のフォルダを飛ばす) へ、↑・PageUp が前の本へ移る。
  - 物理の兄弟・DFS が始まらない (`folder_nav_pending` の mode で確認)。
  - 着地後 nav_stack が `[移動先]` になる。
- 結果一覧の上・結果の画像の全画面: 一覧は何もせず、全画面は既存の案内。物理の移動は始まらない。
- 端: 一覧・全画面とも端の通知で止まる (ループしない)。
- ロック中の継続: タグビューでは物理の継続を返さない。
- 位置の 4: nav_stack の根と表示中のパスが合わないときは、位置が無く案内になる。
- run: BS で結果へ戻った後、旧列の位置は無い。
- 着地後の BS で結果一覧へ戻り、移動先の行が選ばれる。名前が同じ行が 2 つある (親が違う) 場合も、抜けてきた方が選ばれる。
- Folder 行から中の書庫へ着地した場合は、BS 2 回で結果一覧へ戻る。
- 一覧の 5 つの入口 (通常のキー・項目入力停止中のキー・リング/ジェスチャの木と兄弟・ゲームパッドの肩ボタン) のすべてで、
  タグビューの本から結果の順に移り、物理の移動が始まらない。
- 一覧の ListOrder の採用待ち (folder_nav_pending の間と、ZIP の `PhysicalHistoryTransition` の採用待ち) に押しても、次の要求を受け付けない。
  DirectNavigation の走査待ちは、受理で退役する。
- ページ一覧の受付: タグビューで位置があれば受け付け、無ければ受け付けない (ページ一覧は閉じない)。
- nav_stack の上限: 101 段降りても先頭 (根の行) が残り、位置が求まる。
- 既存の §1.444・§1.682 (Folder / Everything の一覧の順) と §1.763 のテストが変わらない。
- テストで実機のキー状態を読む形を新しく書かない (§1.814 (a))。

### 8.2 文書

- [一覧の表示順で前後の本へ移る操作](list-order-book-navigation-plan.md) の §3・§6 に Tag scope を追記する。
- `docs/keymap-spec.md` の Ctrl+↑↓・Ctrl+PageUp/Down の説明、マニュアル (`htdocs/mimageviewer/manual/` のタグビューとショートカットの頁) を更新する。
- 更新履歴の文案 (検収側がブランチの報告に添える):
  「タグビューで開いた本・フォルダから、<kbd>Ctrl</kbd>+<kbd>↑</kbd><kbd>↓</kbd> と <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> で、
  タグの結果の並び順の前後の本へ移るようにしました。これまでは一覧では何もせず、フルスクリーンやゲームパッドの操作では
  タグの結果から外れて、実際のフォルダの前後へ移っていました。タグの結果一覧へ戻ったときは、名前が同じ項目があっても、
  見ていた本が選ばれるようにしました。」
- 別窓の述語・viewport の経路には触れない予定 (独立した別窓はタグビューより先に判定する既存の順のまま)。
  実装で触れることになった場合は、[detached リワーク計画](detached-rework-plan.md) §11 に範囲と理由を記録し、
  ClaudeCode と Codex の双方で「構造的な修正である」ことに合意する。

## 9. レビュー指摘への対応

### 第 1 版 (REVISE、gpt-6.1-sol / xhigh、2026-11-18)

- P2-1 一覧の入口が共通 handler を通らない (項目入力停止中のキー処理とゲームパッドの肩ボタンには、タグビューの判定が無い): 採用。
  5 つの入口を列挙し、一覧用の helper 1 つを同じ位置で呼ぶ (§4.2)。
- P2-2 一覧の ListOrder に、採用待ちの間の拒否が無い: 採用。拒否する期間と、DirectNavigation を拒否の理由にしないことを §4.1 に書いた。
- P2-3 名前による選択では、同名の行を取り違える: 採用。結果一覧へ戻るときだけ、完全なパスで選ぶ (§4.3)。既存の問題も同時に直る。
- P2-4 nav_stack の上限で先頭が消える: 採用。根を残して 2 番目を消す (§3.4)。
- P3-5 Folder 行から中の書庫へ着いた場合は、BS 1 回では戻らない: 採用。期待を「直接その行へ着いた場合」に限り、§5・§8 をそろえた。
- ページ一覧の受付の述語: 採用 (§4.2)。
- レビューで確かめられた前提: 本を開いた後も Search(Tag)、成功採用で nav_stack に積む、空になった BS が再検索、run を実行受付で進めること。
  独立した別窓をタグビューより先に判定する順も、妥当とされた。

### 第 2 版 (ACCEPT、同じレビュー担当、2026-11-18)

- 追加の指摘なし。採用待ちの判定は、`PhysicalHistoryTransition.dfs_continuation` と `MainListAfterHydration::Dfs` に残る移動モードを使う (§4.1 に追記)。
