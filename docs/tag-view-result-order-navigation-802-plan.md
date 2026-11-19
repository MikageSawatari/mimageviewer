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
- 一覧の受付の gate (検収側 r3 の裁定): **タグの一覧の ListOrder は、main の要求がどれか進行中なら拒否する**。
  - 通常 open、ZIP / PDF / 変換書庫の準備、履歴の戻る / 進む、並べ替え・表示条件・監視の更新、
    移動自身の採用待ち・hydration を区別しない。全画面の ListOrder は変更しない。
  - 読むだけの `main_list_request_blocks_grid_list_order` をタグの入口と `start_folder_nav` で共用する。
    既存の退役処理と同じ一時的な要求集合 `MainListRequestKind::ALL` を使う。保存する field・bool・pending は足さない。
    既存の退役方針は保持し、表示更新を新しく取り消す処理は追加しない。
  - 拒否は、既存要求を cancel・再開始せず return し、従来の ListOrder 連打拒否と同じログを残す。
    押した分は累積 (`MAX_PENDING_NAV`) に乗せない。
  - 唯一の例外は §1.805 の DirectNavigation の走査待ち。これだけなら受理し、既存の受付点で退役させる。
    別の要求も同時にあれば拒否する。
  - 一覧の移動の受理で、他の main open を由来・履歴記録方式・intent から分類して退役する処理は置かない。
  - 物理 Grid DFS / SiblingGrid の受理と累積は、このブランチより前の挙動へ戻す。
    先行する通常 ZIP open と物理 Grid DFS の既存の競合は本項の範囲外 (§10 r3)。
  - 逆向きの、通常の明示 open の受理で先の一覧移動を退役する処理は維持する。

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
- 一覧・全画面で、Ctrl+↓・Ctrl+PageDown が結果の次の本 (停止条件で空のフォルダと ZIP / PDF だけを含むフォルダを飛ばす。一覧の順の移動 §1.444 と同じ規則で、中の書庫へは降りない) へ、↑・PageUp が前の本へ移る。
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
- 既存の §1.444・§1.682 (Folder / Everything の一覧の順) と §1.763 のテストは、タグビューの期待を除いて変わらない。
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

## 10. 実装記録 (2026-11-18、fix-tagnav-stack-802-804、未コミット)

### 前提の照合と実装上の具体化

- 設計の主要な前提に矛盾は無かった。`PhysicalHistoryTransition.dfs_continuation` は
  ZIP / PDF / 変換書庫の準備から採用まで残り、採用後は `MainListAfterHydration::Dfs` へ渡る。
  分類待ちの `ClassifiedOpenContinuation::Physical` にも同じ継続が残るため、採用待ち述語はここも見る。
- 普通フォルダの一覧順着地は Grid の install / hydration を使う。hydration が残るときだけ
  既存の `Dfs` 継続 (一覧用・累積0) を載せ、同じ述語で次の要求を拒否する。新しい pending / bool は増やしていない。
  `DirectNavigation` の走査待ちはこの述語の対象外で、既存の `start_folder_nav` の受理点で退役する。
- 完全パスで選ぶ既存処理は場所ジャンプの完了処理にあったが、タグの再検索完了まで要求を運ぶ経路は無かった。
  `child_selection` も名前指定だった。§4.3 の「無ければ」に従い、既存の単一 `select_after_load` と
  `MainListNavigation.selection_after_load` を `AfterLoadSelection::{Name, ExactPath}` に拡張した。
  名前指定の producer / 退避 / 移送 / consumer と既存テストは型追従だけで、名前による選択の挙動を保つ。
- 5 つの一覧入口を確認し、いずれもタグ判定を物理移動より前に同じ helper へ接続した。
  通常キーの解決済み4操作だけを `handle_grid_book_navigation_keys` へ抽出し、実機キー状態を読まずに handler を検査する。
  他入口の★固定・検索などをまとめる router への再編はしていない。
- `take_origin_for_search_entry` が他の検索と★固定を退役させ、`close_searches_for_snapshot` はタグビューを終了する。
  ★固定・Ctrl+G・お気に入り検索とタグビューが同時に有効にならない前提を確認した。
- `FolderNavMode` の全参照をたどり、一覧用だけ `ExplicitList` / Grid 着地とし、従来の全画面操作は
  `fullscreen: true` を明示した。same-kind は ListOrder を引き続き偽、history trigger / perf tag は従来のまま、
  全画面の inflight 継続には ListOrder を加えていない。端では一覧用の通知と全画面の既存 Edge を使う。
- 別窓の述語・viewport 経路、スライドショー、KeyAction の割り当てには変更が無い。

### 文書と検証の範囲

- §8.2 の文書に加え、リンク先のマニュアル `fullscreen.html` と architecture overview の一覧順の節を整合した。
  `next-release-backlog.md` は変更していない。
- 追加の利用者指示に従い、重い処理は `run-heavy.ps1 -Owner codex-802` 経由の bin check と
  filter 付き nextest に限定する。lib 全件 / full gate / dev・release ビルドは検収側へ引き継ぐ。
  アプリの起動・実機入力・コミットは行わない。

### 変更ファイル

| ファイル | 要点 |
| --- | --- |
| `src/tag_view.rs`、`src/app/list_order_navigation.rs` | run / Tag scope、根の行の位置、typed な採用・選択要求、一覧用の共通入口・採用待ち述語 |
| `src/app.rs` | run の更新、根の保持、完全パスの帰路、採用時の根の置換、Grid ListOrder の着地・端・hydration、通常キー・停止中キーの接続 |
| `src/app/gamepad_input.rs` | リング・ジェスチャの木／兄弟、ゲームパッド肩ボタンの接続 |
| `src/ui_fullscreen.rs`、`src/ui_fullscreen/page_overview.rs` | タグの全画面入口、locked 継続、ページ一覧の受付と回帰 |
| `src/app/sidecar_restore.rs`、`src/app/startup_list_restore.rs` | 既存 Dfs 継続による hydration の識別、一覧用 ListOrder の ExplicitList |
| `src/app/close_return.rs`、`src/app/grid_parent.rs`、`src/app/smart_folder.rs`、`src/app/startup_ops.rs`、`src/filename_stack_ui.rs`、`src/ui_dialogs/archive_convert.rs`、`src/ui_dialogs/rename_item.rs`、`src/ui_dialogs/video_upscale.rs` | 既存の名前選択 producer の型追従。既存の名前選択を維持 |
| `src/app/tag_view_list_order_tests.rs` | 新規17件の handler / 状態遷移回帰 |
| `src/app/collection_sibling_navigation_tests.rs` | 利用者承認による既存 §1.763 のタグケース更新・改名。通常フォルダ・閲覧履歴の検査は維持 |
| `src/app/close_return_tests.rs`、`src/app/collection_grid.rs`、`src/app/list_order_everything_tests.rs`、`src/app/list_order_worker_tests.rs`、`src/app/location_jump.rs`、`src/app/tests.rs`、`src/app/tests/section1795.rs`、`src/app/tests/startup_restore.rs`、`src/ui_main.rs` | 既存テストの型追従。タグ結果への BS の期待値だけ名前から完全パスへ変更 |
| `docs/list-order-book-navigation-plan.md`、`docs/keymap-spec.md`、`docs/architecture-overview.md`、本書 | Tag scope と操作・実装記録 |
| `htdocs/mimageviewer/manual/tags.html`、`shortcuts.html`、`fullscreen.html` | 結果順の本移動、結果一覧での no-op、戻り方と選択 |

### 5 つの一覧入口

| 入口 | 実装と直接回帰 |
| --- | --- |
| 通常キー | 既存 KeyAction の解決後、`handle_grid_book_navigation_keys` → 共通タグ helper。4操作・両方向を検査 |
| 項目入力停止中のキー | `handle_stack_refresh_keyboard` の4操作のループ → 共通タグ helper。合成 egui / native frame で検査 |
| リング・ジェスチャの木 | `handle_grid_tree_folder_nav` → 共通タグ helper。両入口・両方向を検査 |
| リング・ジェスチャの兄弟 | `apply_sibling_folder_nav` の一覧側 → 共通タグ helper。両入口・両方向を検査 |
| ゲームパッド肩ボタン | `handle_gamepad_grid_folder_nav` → 共通タグ helper。両方向を検査 |

いずれもタグの本では `ListOrder { fullscreen: false }`、結果一覧では取消のみ、位置不明では通知のみ。
ZIP 内 DFS・物理 DFS・兄弟移動へは落ちない。ほかの文脈の既存の分岐順は維持する。

### 新規テスト名

`src/app/tag_view_list_order_tests.rs` (17件):

- `tag_list_order_capture_only_result_books_and_preserve_nested_snapshot`
- `tag_list_order_grid_and_fullscreen_handlers_follow_results_in_both_directions`
- `tag_list_order_root_absent_keeps_result_grid_and_shows_existing_fullscreen_guidance`
- `tag_list_order_invalid_root_path_and_run_never_fall_back_to_physical_navigation`
- `tag_list_order_boundary_does_not_wrap_or_replace_the_root`
- `tag_list_order_adoption_replaces_only_matching_run_and_row`
- `tag_list_order_back_requeries_and_selects_exact_same_named_row`
- `tag_list_order_deep_descents_keep_original_root_even_when_nested_rows_are_results`
- `tag_list_order_grid_pending_rejects_without_cancel_restart_or_accumulation`
- `tag_list_order_grid_zip_adoption_gate_keeps_owner_and_commits_new_root`
- `tag_list_order_grid_folder_hydration_gate_keeps_owner_until_completion`
- `tag_list_order_cancel_and_scan_failure_preserve_the_opened_root`
- `tag_list_order_grid_request_supersedes_direct_navigation_scan`
- `tag_list_order_folder_then_archive_returns_through_folder_before_results`
- `tag_list_order_normal_keyboard_tree_and_sibling_use_result_order`
- `tag_list_order_ring_gesture_tree_sibling_and_gamepad_all_use_result_order`
- `tag_list_order_blocked_item_keyboard_tree_and_sibling_use_result_order`

`src/ui_fullscreen/page_overview.rs` (1件):

- `tag_view_page_overview_acceptance_matches_result_position_and_rejects_locked_continuation`

### 独立レビューと反復検証の記録

- 独立ソースレビュー (`gpt-6.1-sol` / `xhigh`) は **ACCEPT**。
  最初に指摘された一覧端通知の誤文字化は修正し、普通フォルダ hydration の直接回帰も追加した。
  別窓の述語 / viewport の差分が無いこと、既存の Folder / Everything の全画面 ListOrder の維持も確認された。
  reviewer は Cargo を実行していない。
- 新規テストの初回コンパイルは **exit 101**。テストコードの `RawInput.modifiers` (現 API に無い) と
  ページ一覧テストの非公開メソッド参照の2件で、`ModifiersChanged` イベントと既存の
  `virtual_list_context_id()` を使う形に訂正した。ログ: `target/802-narrow.log`。
- 訂正後の新規18件は **17成功 / 1失敗、exit 100** (test compile 9m18s、実行6.722s)。
  ログ: `target/802-narrow-fixed.log`。失敗は停止中キー入口のテストが App pass の keyboard owner proof を
  供給しておらず、Keymap が fail-closed で拒否したもの。製品コードは変更せず、合成
  `KeyboardOwnershipSnapshot` → `decide_keyboard_owner` → `cache_keyboard_owner` をテストへ追加した。
  reviewer はこの修正も ACCEPT とし、実機キー状態の読取りを追加していないことを確認した。
- 重いコマンドのロック領域へ書けない場合は run-heavy の既定の BelowNormal / 6並列で絞ったものだけ実行した。
  別担当の full-test ロックがある場合は、迂回せず run-heavy の待機に従った。

### 関連検証と設計の検証条件の不整合 (2026-11-19、当初は判断待ちで停止)

| 確認 | 結果 |
| --- | --- |
| 最終差分の `cargo check -p mimageviewer --bin mimageviewer-core` | **exit 0**、35.88s。`target/802-check-final.log` |
| 関連 nextest | **922件実行、921成功 (うち1 leaky) / 1失敗、exit 100**。compile 2m50s、実行100.345s。`target/802-related.log` |
| 新規18件 | 上の関連実行にすべて含まれ、**18件成功**。合成 keyboard owner proof の訂正後は停止中キー入口も成功 |
| `cargo fmt` (引数なし)、最終 `cargo fmt --check` | 成功 |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 危険文字0 / 成功 |
| lib 全件 / full gate / dev・release ビルド / 実アプリ操作 / commit | 利用者指示により実行していない |

関連実行は、指定の filter に名前選択の型追従の影響先を追加したもの。重いコマンドはいずれも
指定の `run-heavy.ps1 -Owner codex-802 -WorkDir G:\home\mimageviewer-listnav -Command '<cargo command>'` 経由。

```text
cargo nextest run -p mimageviewer --lib -E "test(/tag_view|list_order|section1763|section257|favsearch|grid_parent|folder_nav|page_overview|gamepad|close_return|select_after_load|section1795|section1672|startup_restore|direct_navigation_worker/)"
```

失敗は既存 `section1763_physical_tag_and_reading_history_keep_sibling_routes`
(`src/app/collection_sibling_navigation_tests.rs`) のタグ全画面ケースだけ。
このケースは `tag_view.active = true`・空の nav_stack で **物理 `SiblingFullscreen` の開始**を要求する。
一方、本書冒頭・§2・§4.2 はその物理への脱出を廃止し、この状態では案内のみとする。
したがって、§8.1 の「既存の §1.763 のテストが変わらない」と、新しい動作の期待値が両立しない。
同じテスト内の通常フォルダ・履歴の経路は今回変えていない。

独立 reviewer はこの不整合を確認し、**製品コードの ACCEPT は維持、設計の検証条件は REVISE** とした。
利用者の「設計と矛盾していれば、黙って変えずに止めて報告する」に従い、旧テストの期待値は変更していない。
製品側へ物理移動を復活させる guard は追加しない。

停止時に提示した最小案は「§1.763 の通常フォルダ・履歴の回帰は維持し、タグの旧期待値だけを §1.802 の
結果順／根なし時の案内へ更新する」と §8.1 を明確化すること。今回の実行結果を隠す期待値緩和ではなく、
明示的に廃止する旧仕様と新仕様の検証範囲を一致させるための変更であり、判断前には実施せず利用者へ確認した。

### §8.1 の訂正と既存タグ回帰の更新 (2026-11-19、利用者承認)

利用者より、§8.1 の「§1.763 のテストが変わらない」は検収側の書き誤りであり、
通常フォルダ・閲覧履歴の期待は維持し、タグ部分だけを新仕様へ更新して続行するよう明示指示を受けた。
§8.1 を「タグビューの期待を除いて」に訂正した。上の停止と失敗結果は、その判断前の記録として残す。

- `section1763_physical_tag_and_reading_history_keep_sibling_routes` を
  `section1763_physical_and_history_keep_sibling_routes_tag_books_follow_results` へ改名した。
- 通常フォルダ・閲覧履歴は、従来の一覧 / 全画面の入力入口、物理 Sibling モードと移動先の期待をそのまま維持した。
- タグ部分だけは正規の `Search(Tag)` surface・run・根の行・Tag scope の列を持つ本の状態にし、
  物理兄弟とは異なる親にある結果行を次の行として配置した。一覧 / 全画面の兄弟操作がともに
  `ListOrder` を作り、物理兄弟へ移らずその結果行へ進むことを検査する。
  一覧側は解決済み操作の handler を使い、実機キー状態の読取りを追加していない。
- 結果一覧上 / 根なし時の no-op・案内は、新規の §1.802 専用回帰で引き続き検査する。
- 製品コードの変更は無い。今回追加で変更したファイルは、この設計書と
  `src/app/collection_sibling_navigation_tests.rs` の2ファイルだけ。

### 訂正後の最終確認 (2026-11-19、完了・未コミット)

| 確認 | 結果・証跡 |
| --- | --- |
| 上記と同じ関連 nextest filter | **exit 0、922件実行 / 922成功 (2 leaky) / 失敗0、18,008 skip**。compile 4m50s、実行105.857s。`target/802-related-approved.log`、JUnit控え `target/802-related-approved-junit.xml` |
| 新規18件と更新した §1.763 の1件 | すべて上の922件に含み成功。§1.763 の1件は通常フォルダ・閲覧履歴・タグ × 一覧 / 全画面の6ケースを検査 |
| `cargo fmt` (引数なし) / `cargo fmt --check` / `git diff --check` | すべて成功 |
| bin check | 前回最終の **exit 0** (`target/802-check-final.log`) を再利用。製品コードはその後変更しておらず、今回の Rust 差分は `cfg(test)` 内の既存テストだけ |
| UI glyph | 前回最終の危険文字0を再利用。UI文字列に追加変更なし |
| 今回の2ファイルの独立レビュー (`gpt-6.1-sol` / `xhigh`) | **ACCEPT**。非Tagの検査維持、Tagの結果行 / ListOrder と物理兄弟への非脱出、実機キー読取りなし、設計訂正と承認記録の整合を確認。reviewer は Cargo を実行せず親の実行結果を使用 |

nextest の leaky 2件の注記は上表のとおり残し、失敗0と区別する。
重い処理は引き続き指定の run-heavy 経由の絞った nextest のみ追加し、全件 / full gate / dev・release ビルドは行っていない。
別ブランチ・別作業ツリー・`next-release-backlog.md` に変更は無く、アプリ起動・コミットも行っていない。

### 実装レビュー r1 の P2 / P3 対応 (2026-11-19)

利用者は `c96d81ce3` の独立コードレビュー r1 の2指摘を正しいと判断し、次の修正を指示した。
直前の `e836ad36f` (§1.804) の変更範囲には触れない。

- **P2 の原因と Grid DFS の照合**: `start_folder_nav` は先行する通常 ZIP 等の main preflight を退役せず、
  通常 open の受付も `folder_nav_pending` を退役しなかった。物理の Grid DFS も同じ受付を通り、同じ競合があった。
  先行する通常 open が採用されて後発の移動 worker を消す順と、先行する移動が後発の open を退役させる順の両方を修正する。
- **受理境界での後勝ち**: 新規の `Grid` / `SiblingGrid` / `ListOrder { fullscreen: false }` は、
  既存の拒否・同モードの累積判定の後、spawn の前に `retire_replaced_main_list_requests(None)` を使って先の main open を退役する。
  新 worker はまだ存在しないので、保持用の新 owner・pending・bool は不要。DirectNavigation も同じ受理点で退役する。
- 通常 main open の共通退役と成功した claim の main / Admission / explicit 受付に、旧 Grid 系 worker の退役を接続した。
  worker の取消は既存 `cancel_inflight_order_dependent_folder_nav` を使い、分類 / Physical preflight は従来の main owner 退役を使う。
  採用済み一覧の hydration は維持し、その中の古い `Dfs` (Grid 系) の移動継続だけを外す。
  自動 refresh / resume / format 継続は `explicit_request == false` のまま、追加した Grid 系の退役 helper を発火させない。
- **main 行きの確定点**: `GridFolderCandidate` が既存の `!should_detach` により main 行きと確定した後も旧 Grid を退役する。
  ここは初回 scan で startup の受付が済んでいるので既存の `false` を維持し、main navigation の退役だけを明示した。
  should_detach の計算・選別、非 main 側、shell / lease / viewport の処理は変更していない。
- ListOrder 同士の準備 / 採用待ちの拒否、Grid DFS の既存の連打累積、Fullscreen 系の sequence / 継続契約は維持する。
  実装担当内の別 context による source 監査では、refresh を新 open として取り消さない条件と、この限定した main 確定点の処理を確認した。
  この確認は検収側の独立レビューとは別であり、後の検収側 r2 は下記2件を指摘して REVISE とした。
- **P3**: `tag_list_order_folder_then_archive_returns_through_folder_before_results` は admission 直渡しを撤去した。
  本 A / stack `[A]` から実際の全画面 ListOrder → worker → Folder 行の採用 → hydration → 内部 ZIP の open / 採用まで通し、
  `[Folder 行, ZIP]` を検査する。音声 + 有効 PNG 入り ZIP の Folder を用い、FS 再開後に一覧へ戻して BS 2回、
  run の維持 / 更新・完全パスの選択要求・実際のタグ結果再採用と行選択まで検査する。

変更ファイルは `src/app.rs`、`src/app/sidecar_restore.rs`、`src/app/tag_view_list_order_tests.rs`、
新規 `src/app/tag_view_open_supersession_tests.rs`、本書。
§1.804 の StackSelection / スタックカーソル・スクロールの処理、別窓の述語 / viewport は変更しない。

追加した P2 回帰 (実 worker の結果と `PhysicalHistoryPreflight::controlled_for_test` の channel で採用を留める):

- `tag_view_open_supersession_grid_acceptance_retires_older_ordinary_zip_preflight`
- `tag_view_open_supersession_ordinary_zip_retires_pending_and_completed_grid_workers`
- `tag_view_open_supersession_ordinary_zip_retires_grid_dfs_staged_zip_adoption`
- `tag_view_open_supersession_automatic_refresh_claim_and_retirement_keep_grid_worker`
- `tag_view_open_supersession_refused_ordinary_zip_keeps_grid_worker`
- `tag_view_open_supersession_ordinary_zip_retires_grid_hydration_tail_only`
- `tag_view_open_supersession_grid_candidate_main_fallback_retires_grid_worker`

通常 open → Grid、Grid → 通常 open、worker 完了済み / ZIP 採用待ちの両状態を Tag ListOrder と物理 Grid DFS で検査する。
普通 Folder の Grid DFS は従来 Dfs tail を持たないため、普通 Folder の hydration 内の Dfs 退役は Tag ListOrder で検査する。
candidate の回帰は未登録の fake lease を使い、実窓を生成しない。受付前に拒否された open と自動 refresh の claim が旧 worker を壊さない負例も含む。

**refresh の検査範囲**: 上の負例は refresh の claim / 共通退役で、今回追加した worker 取消が発火しないことを検査する。
監視 refresh の全 lifecycle で移動を保持する修正ではない。独立監査と source 照合により、物理 Grid DFS の ZIP が
staged 採用待ちの間に、元の通常フォルダの監視走査が `RefreshList { automatic_observation: true }` で採用へ進むと、
既存の claim の history 退役 (`claim_open_request_owner_with_startup_intent` 後段) が Physical transition を外し得ることも見つかった。
`folder_observation_allowed` は通常の物理一覧では有効で、タグの結果 scope では無効である。
この既存の監視更新との競合は今回の explicit な通常 open との両順の競合とは分け、範囲外として残す。
今回の helper はその false 経路では発火せず、この既存挙動を拡大していない。実機での観測は無い。

#### r1 の検証記録

- 初回の絞った nextest は、テスト実行前に rustc が `0xffffffff` で終了した (`target/802-r1-focused.log`、run-heavy exit -1)。
  Rust の型エラーやテスト失敗の診断は無く、終了原因は特定していない。成功・テスト失敗の件数には数えない。
- その後の `cargo check -p mimageviewer --bin mimageviewer-core` は **exit 0、34.47s** (`target/802-r1-check.log`)。
- 同じ絞った nextest を一度再実行し、**8件実行 / 8成功 / 失敗0、exit 0** (compile 9m42s、実行1.875s)。
  `target/802-r1-focused-retry.log`、JUnit控え `target/802-r1-focused-junit.xml`。
  新規 P2 の7件と、実経路へ置き換えた P3 の既存1件を含む。
- 関連 filter の nextest は **934件実行 / 934成功 / 失敗0、exit 0** (compile 2.22s、実行103.100s、18005件除外)。
  nextest の leaky 判定は2件。`target/802-r1-related.log`、JUnit控え `target/802-r1-related-junit.xml`。
  filter は `tag_view|list_order|section1763|section257|favsearch|grid_parent|folder_nav|page_overview|gamepad|close_return|select_after_load|section1795|section1804|section1672|startup_restore|direct_navigation_worker|main_list`。
  新規7件と P3 の既存1件もこの934件に含むため、成功件数を重複加算しない。
- `cargo fmt` (引数なし)、`cargo fmt --check`、`git diff --check` は成功。UI文字列の変更は無い。
- 実装担当内の別 context によるコード確認 (`gpt-6.1-sol` / `xhigh`) は **ACCEPT** とした。
  検収側の独立レビューとは別の確認であり、検収側の r2 は **REVISE**。下記の2退行をこの内部確認では検出できなかった。
  explicit な通常 open の両順、main/context と Grid mode の限定、実 hydration / Fullscreen の保持、
  P3 の実採用・BS・再検索、refresh の既存事象の切り分けを確認。reviewer は Cargo を実行せず実装担当の証拠を使う。
- 重い処理は今回も指定の run-heavy 経由の bin check と filter 付き nextest のみ。
  ロックを書けない場合は既定の BelowNormal / 6並列で絞ったものだけ実行し、full-test ロックがある場合は迂回せず待機した。
  全件 / full gate / dev・release ビルド / 実アプリ操作 / commit は行っていない。

### 検収側の独立レビュー r2 の P2 2件への対応 (2026-11-19)

利用者は `bed81647e` に対する検収側の独立レビュー r2 の2指摘を正しいと判断した。
直前の `3c8cc77fe` (§1.804) の変更範囲には触れない。

- **原因**: Grid 系移動の受付で `retire_replaced_main_list_requests(None)` を丸ごと呼んだため、
  Grid DFS 自身の ZIP 採用待ちと、同じ一覧の `CurrentViewOrderRefresh` まで退役していた。
  前者は同じキーの再入力で preflight を捨てて準備し直し、後者は兄弟移動が端で終わると新設定と旧表示の不一致を残した。
- **対象を限定**: `start_folder_nav` は `retire_explicit_main_opens_for_grid_traversal` へ置き換える。
  既存 request の型だけを読み、別の明示 main open を退役する。新しい field・pending・bool は足さない。
  共通退役が行う tag/rating/collection/subfolder 等の一括退役、sidecar hydration や移動継続の解除は行わない。

| 対象 | 今回の受理点で退役する要求 |
| --- | --- |
| パス分類 | main の通常 direct / SmartGrid / Physical open。生の `restore_intent` が `ExplicitList` / `ClassifyFolder` / `PageContinuation`。Physical は `dfs_continuation: None` のみ |
| Physical preflight・変換待ち | main の上記明示 intent で、`dfs_continuation: None` の要求のみ |
| pane scan | `PaneNavigation` / `DirectNavigation` / `JumpToPhysicalFolder` / `RequiredFullscreenTarget`。`CurrentViewOrderRefresh`、行先未確定 candidate、別窓目的は保持 |
| 直接 PDF | 上記明示 intent の `ColdCandidate` のみ。`CommittedVerification` と retained source は保持 |

分類と Physical の `SameLocation` / `Replay` は対象外にする。タグや検索内の通常 open は既存経路で
`MainHistoryOperation::Restore` になるため、`Direct` だけの判定にはしない。
`replaces_deferred_startup_open()` は `InternalHydration` の内側と `RestoreList` も明示扱いにするので使わず、生の variant を判定する。
分類の `SnapshotArchiveCacheOnly` (手動 / 全画面 / slideshow が混在)、行先未確定の `BookmarkRow`、Collection 履歴要求は変更しない。
既存 owner が別窓を指す分類 / PDF 要求は退役対象にしない。別窓の行先を決める既存述語や viewport の処理は変更していない。

PDF 固有の既存 cleanup を `retire_cold_pdf_candidate` へ等価に分け、限定退役からはそれだけを呼ぶ。
従来の共通退役側は従来どおり deferred book open も退役するが、今回の限定退役はそれを巻き込まない。
PDF の source 復帰・EPUB address の復元は既存処理を使う。

ListOrder 同士の拒否述語は変更しない。物理 Grid / SiblingGrid の ZIP 採用待ち中の再入力は、`bed81647e` 前と同じく
元の Physical 要求を保持して worker を開始し、その worker への同種再入力を既存 accumulator に加える。
新しい採用待ちの累積方式や再試行は導入しない。§1.805 の DirectNavigation scan の退役、
通常 open の受理で古い Grid 系移動を退役する逆向きの処理も維持する。

追加した回帰:

- `tag_view_open_supersession_physical_repeat_keeps_its_staged_zip_preflight`:
  Grid / SiblingGrid の実 handler と worker で B.zip の採用待ちまで進め、制御用 preflight を保持。
  再入力後の request ID・receiver・queued steps と既存の累積、元の結果による実採用を検査する。
- `tag_view_open_supersession_sibling_edge_keeps_real_current_view_order_refresh`:
  実 sort refresh scan の結果を channel で保持し、端で終わる兄弟移動後も取消されず、新しい日付順の表示へ採用されることを検査する。

変更ファイルは `src/app.rs`、`src/app/tag_view_open_supersession_tests.rs`、本書。
r1 の既存7件、P3 の実経路テスト、ListOrder の連打拒否テストは変更しない。
本修正は退役対象を既存 owner 境界で区別するもので、操作のモーダル化や待機・retry・追加 repaint は不要。
まれな失敗向けの処理は追加しない。

#### r2 の検証記録

- `cargo check -p mimageviewer --bin mimageviewer-core` は **exit 0、29.92s** (`target/802-r2-check.log`)。
- 初回の絞った nextest は12件中11成功、並べ替え更新の新規1件が最終行順の検査で失敗した。
  要求の保持・受け渡しは成功していた。fixture が共通設定の製本 root (`tmp/books`) 直下にあり、
  `book_materialization` の製本仕様による数字順固定を受けていたのが原因。
  通常の物理フォルダ (`tmp/library/A`) へ移し、非製本の前提と DateDesc 設定の保持も断言する。
  新しい日付順で表示される期待は維持し、製品コードの追加変更は行わない。
  初回ログ `target/802-r2-focused.log`、JUnit控え `target/802-r2-focused-initial-junit.xml`。
- fixture 修正後の同じ filter は **12件実行 / 12成功 / 失敗0、exit 0** (compile 2m42s、実行2.284s)。
  `target/802-r2-focused-retry.log`、JUnit控え `target/802-r2-focused-junit.xml`。
  r1 の7件・P3 の1件・今回の2件に加え、ListOrder の連打拒否
  `tag_list_order_grid_pending_rejects_without_cancel_restart_or_accumulation` と
  `tag_list_order_grid_zip_adoption_gate_keeps_owner_and_commits_new_root` も成功した。
- `cargo fmt` (引数なし)、最終の `cargo fmt --check`、`git diff --check` は成功。UI文字列の変更は無い。
- 実装担当内の別 context (`gpt-6.1-sol` / `xhigh`) による型・差分と fixture 修正の確認では、追加の修正指摘は無かった。
  検収側の独立レビューとは別の確認。担当者は Cargo を実行せず、実行結果は実装担当が集約する。
- 前回と同じ関連 filter の nextest は **938件実行 / 938成功 / 失敗0、exit 0** (compile 1.23s、実行92.130s、18005件除外)。
  leaky 判定は0件。上の12件も含むことを JUnit で確認し、件数を重複加算しない。
  `target/802-r2-related.log`、JUnit控え `target/802-r2-related-junit.xml`。
  filter は `tag_view|list_order|section1763|section257|favsearch|grid_parent|folder_nav|page_overview|gamepad|close_return|select_after_load|section1795|section1804|section1672|startup_restore|direct_navigation_worker|main_list`。
- 重い処理はすべて指定の `run-heavy.ps1 -Owner codex-802` 経由の bin check と filter 付き nextest のみ。
  共通ロックを書けないため runner の BelowNormal / 6並列で実行した。
  全件 / test-full / dev・release ビルド / 実アプリ操作 / commit は行っていない。

### 検収側の独立レビュー r3: 後勝ちの分類をやめて拒否へ寄せる (2026-11-19)

利用者は `622c53e98` の `Replay` 除外による競合を正しい指摘と判断し、受付の規則そのものを変更した。
`MainHistoryOperation` は履歴の記録方式であり、明示入力の由来ではない。
r1・r2 の後勝ちの分類が組み合わせごとに漏れたため、CLAUDE.md の「状態の組み合わせを減らす」に従い、
**タグの一覧の ListOrder は main の要求が進行中なら拒否する**。r1・r2 の記録は履歴として残し、現行仕様は本節と §4.1 とする。

- `retire_explicit_main_opens_for_grid_traversal` と、その手書きの intent / history / owner 分類を全削除。
  他の利用が無くなった `retire_cold_pdf_candidate` の切り出しも元へ戻した。
- `MainListRequestKind::ALL` は保存状態を持たない要求集合。既存の退役と読むだけの
  `main_list_request_blocks_grid_list_order` が同じ集合を使い、タグの入口と共通 start が同じ述語を呼ぶ。
  活動中かだけを読み、`Replay`・通常 open・同じ移動の継続を分類しない。
- 集合は pane close / Grid worker / 未解決 startup・activation / close return / tag / 分類 / Physical 履歴 / rating /
  pane scan / Smart / Collection / subfolder / bookmark / document 準備に加え、hydration・reload・main の外部再走査・現在地の保留更新。
  通常の ZIP / PDF / 変換書庫は既存の分類・Physical 要求を介して準備・変換中も集合に残る。
- 退役側は従来と同じ順・retained / explicit 条件で処理する。活動中かで退役処理を省略しない。
  新たに拒否対象にする表示更新は退役では何もしないため、更新取消や逆向き受付の挙動を広げない。
  startup の main 所有と pane close の既存条件は、読む側と退役・消費側で共用する小さな述語へ抽出した。
- 唯一の例外は DirectNavigation の pane scan。他の要求が無ければ ListOrder を受理し、§1.805 の既存の受付点で scan を退役する。
  他の項目も進行中なら、その項目により拒否される。拒否は cancel・再準備・押下の累積をせず、従来と同じログで終了する。
- hydration は既存の projected-context 述語を使い、外部再走査は main owner だけを見る。
  旧フォルダ向けに休止している保留更新を永久に拒否理由にしないよう、更新の消費側と共有した現在の適用先を確認する。
  常駐する監視 subscription は要求ではなく、走らせ中の再走査・現在地の更新だけが対象。

**物理 Grid DFS / SiblingGrid** の受理では新しい退役を行わず、このブランチより前の挙動へ戻した。
DirectNavigation scan の退役と従来の累積は維持する。先に受理した通常 ZIP open が後から Grid DFS に勝つ競合は、
出荷済みの既存の競合として本項の範囲外にする。修正する場合は別項目とする。
通常の明示 open の受理で進行中の一覧移動を退役する逆向きの helper は維持する。
全画面の ListOrder、§1.804、別窓の行先述語・viewport は変更しない。

**テストの変更・追加**:

- r1 の `tag_view_open_supersession_grid_acceptance_retires_older_ordinary_zip_preflight` を
  `tag_view_open_supersession_tag_refuses_older_ordinary_zip_preflight` へ置換。Tag は拒否し、元の ZIP が着地する期待へ変更。
  物理 Grid が旧 ZIP を退役する arm は、今回の範囲外の競合を修正する期待だったため削除。
- `tag_view_open_supersession_tag_refuses_back_and_forward_zip_replay_preflight` を追加。
  履歴キーの availability は変更せず、既存の `FolderHistoryPlan` と typed dispatch で受理済み Replay を作り、
  戻る・進む両方の ZIP preflight を制御して、ListOrder 拒否後も元の要求が採用されることを検査する。
- `tag_view_open_supersession_tag_refuses_current_view_order_refresh_and_keeps_new_order` を追加。
  実 scan を channel で保持し、拒否後の元要求の継続と新しい日付順への採用を検査する。
- `tag_view_open_supersession_tag_accepts_and_retires_direct_navigation_scan` を追加。
  DirectNavigation の実 scan を保持し、ListOrder の受理・旧 scan の取消・結果の次の本への採用を検査する。
- r1 の `ordinary_zip_retires_grid_hydration_tail_only` には、自身の hydration 中の ListOrder 拒否と owner 保持を追加。
  その後の通常 open による逆向き退役の検査は維持する。ほかの r1 の逆向き・負例、r2 の物理 Grid 2件、P3、連打拒否は維持する。
  共通 fixture は初回 hydration を完了してからキー受付を検査する。

マニュアル `tags.html` に、一覧では本を開く準備中や一覧の更新中も受け付けない旨を追記した。
`docs/keymap-spec.md` のタグ一覧の受付契約も同じ規則へ揃えた。
新しい保存 field・pending・bool、retry・delay・追加 repaint は無い。まれな失敗の回復処理も追加しない。

App-global な `archive_convert` の単純な有無は main の要求ではないため、別窓の変換を拒否理由にしない。
main のタグ由来の通常変換は既存の分類から Physical 要求を持ち、変換・再 preflight までその所有が続く。
Smart 由来も既存の transition が残る。この所有境界を使い、変換の由来を新しく分類する helper は設けない。

#### r3 の検証記録

- 指定 run-heavy 経由の `cargo check -p mimageviewer --bin mimageviewer-core` は **exit 0、1m32s** (`target/802-r3-check.log`)。
- `cargo fmt` (引数なし)、`cargo fmt --check`、`git diff --check` は成功。
  マニュアル追記後の `python scripts/check_ui_glyphs.py` は危険なグリフ0件。
- 実装担当内の別 context (`gpt-6.1-sol` / `xhigh`) で、共有集合、退役側の順序と条件、main 所有、
  DirectNavigation の例外、既存の物理移動への復帰を確認した。App-global な archive 変換を単純な有無で読む点を指摘対応し、未解決の指摘は無い。
  検収側の独立レビューとは別の確認で、Cargo は実装担当だけが実行する。
- 初回の focused nextest は **15件中14成功、DirectNavigation の新規1件が準備時点で失敗** (compile 9m22s、実行2.088s)。
  テストが同期 open wrapper を呼び、走査待ちの入口を通していなかった。
  既存の §1.805 テストと同じ `open_direct_navigation_target_at_stage(..., DirectoryScanRoute::Worker)` へ修正し、
  元の本の表示と実 pending の保持も検査する。製品コードの追加変更は無い。
  `target/802-r3-focused.log`、JUnit控え `target/802-r3-focused-initial-junit.xml`。
- テスト入口修正後は **15件実行 / 15成功 / 失敗0、exit 0** (compile 2m33s、実行2.071s)。
  `target/802-r3-focused-retry.log`、JUnit控え `target/802-r3-focused-junit.xml`。
  r1 の逆向き・負例と今回の拒否 / 例外、r2 の物理 Grid 2件、P3、既存の ListOrder 連打拒否2件を含む。
- 前回と同じ関連 filter の nextest は **941件実行 / 941成功 / 失敗0、exit 0** (compile 1.15s、実行92.681s、18005件除外)。
  nextest の leaky 判定は2件。focused の全15件も含むことを JUnit で確認し、件数を重複加算しない。
  `target/802-r3-related.log`、JUnit控え `target/802-r3-related-junit.xml`。
  filter は `tag_view|list_order|section1763|section257|favsearch|grid_parent|folder_nav|page_overview|gamepad|close_return|select_after_load|section1795|section1804|section1672|startup_restore|direct_navigation_worker|main_list`。
- 重い処理は指定の `run-heavy.ps1 -Owner codex-802` 経由の bin check と filter 付き nextest だけ。
  full-test ロックは迂回せず待ち、ロック書込不可時は runner の BelowNormal / 6並列で実行した。
  全件 / test-full / dev・release ビルド / 実アプリ操作 / commit は行っていない。

### r4 ACCEPT 後の全件テストで見つかった分類受付時点の退行 (2026-11-19)

`a740f6525` は検収側の独立レビュー r4 で ACCEPT だったが、検収側の全件テストで既存の
`accepted_epub_retires_pending_dfs_after_classification_but_ignore_keeps_it` が失敗した。

**原因**: 「拡張子の分類はまだファイルの受付ではない」という見立ては正しかった。
ただし報告された Physical 経路の直接の取消箇所は claim ではなく、分類受付が
`retire_replaced_main_list_requests_with_startup_intent(Some(Classification), true)` を呼び、
共通退役の `GridTraversal` が既存 worker / hydration の移動継続を消す点だった。
Ignore の判定は分類結果の poll 後なので、ここで退役すると拒否された候補のために旧移動まで失われる。

**修正**:

- 共通退役の Grid 項目は `retained == Classification` の間は実行しない。
  cold PDF を保持する既存の DirectDocument 項目と同じく、分類結果前は旧移動を保持する。
- Physical の preflight / lease が受理された既存の native 境界で、`Continuation` でも逆向き退役を行う。
  既存 `dfs_continuation` がある自身の移動と、明示 open ではない refresh は、この追加退役の対象にしない。
- 同じ分類受付を共有する Smart と Collection も、準備を受理した native 境界で退役する。
  Smart の分類完了関数は Ignore / 準備失敗でも `true` を返すため、その戻り値を受理と解釈せず、実際の準備成功後へ置いた。
- claim の `Admission` 制限は保持する。自身の DFS も `true, Continuation` で claim するため、
  制限を外すと自身の移動を取り消し得る。分類完了を `Admission` へ戻すこともしない。
  これにより、後発 activation を分類の継続が退役させない既存の契約も保つ。
- metadata で実フォルダと分かった Direct 系の分類は、同じ継続呼出し内の既存 install が旧 worker を退役する。
  この経路専用の stage / owner 分類は追加しない。以前からある EPUB 固有の取消処理も変更しない。

ZIP / PDF は現行の `path_needs_open_classification` の対象外であり、非分類の明示 open として従来どおり受理時に退役する。
EPUB と変換書庫は分類中に保持し、Ignore なら旧移動を保持、受け付けた続きで退役する。
新しい pending・bool・stage variant は無い。r3 の ListOrder 拒否規則・DirectNavigation 例外、物理 Grid の範囲外の競合は変更しない。
§1.804 と、検収側の未コミット `docs/next-release-backlog.md` は変更しない。

**回帰**:

- 既存失敗テストに、Ignore / Ask の両方で分類開始直後の同じ cancel token・受信口・累積2ステップの保持を追加。
  元の Ignore 後保持 / Ask 後退役の期待は維持する。
- `tag_view_open_supersession_owned_classified_files_retire_after_classification_zip_pdf_on_admission`:
  Grid DFS / Tag ListOrder の両方で、EPUB・7z は分類中保持→受理後退役、ZIP / PDF は非分類の受理時退役。
  実 worker の旧結果を channel に保持し、受信口の存続 / 破棄も検査する。PDF の native enumeration は poll しない。
- `tag_view_open_supersession_owned_ignored_classification_keeps_grid_worker_and_result`:
  Ignore の EPUB / 7z に拒否された後、元の実 worker 結果が B へ着地できることを両方の移動で検査する。
- `tag_view_open_supersession_owned_suffix_directory_retires_grid_only_after_classification`:
  `.epub` / `.7z` という名前の実フォルダは Ignore でも分類後に採用し、その時点で旧移動を退役する。

**検証記録**:

- 指定 run-heavy 経由の `cargo check -p mimageviewer --bin mimageviewer-core`: **exit 0、24.08s**。
  `target/802-classification-check.log`。
- focused nextest: **21件実行 / 21成功 / 失敗0、exit 0** (compile 3m06s、実行3.551s)。
  報告された既存失敗テスト、上記3件、r1〜r3 の拒否・逆向き退役・DirectNavigation、r2 の物理 Grid 2件、P3、連打拒否、
  `section1285_classification_handoffs_preserve_later_activation` を含む。
  `target/802-classification-focused.log`、JUnit控え `target/802-classification-focused-junit.xml`。
- 拡張した関連 nextest: **1343件実行 / 1343成功 / 失敗0、exit 0** (compile 1.11s、実行110.440s、17606件除外)。
  nextest の leaky 判定は1件。focused の全21件も含むことを JUnit で確認し、件数を重複加算しない。
  前回の filter に `accepted_epub|classification|dfs|pdf_enumerate|epub` を追加 (`folder_nav` は前回から含む)。
  `target/802-classification-related.log`、JUnit控え `target/802-classification-related-junit.xml`。
- `cargo fmt` (引数なし)、`cargo fmt --check`、`git diff --check` は成功。UI文字列の変更は無い。
- 実装担当内の別 context (`gpt-6.1-sol` / `xhigh`) で各分類入口・受理境界・自身の継続・refresh を照合した。
  未解決の指摘は無い。正式な検収側の独立レビューとは別の確認で、Cargo は実装担当だけが実行した。
- 検収側バックログの未コミット内容は開始時・終了時の SHA256 一致で確認した。
  重い処理は指定の run-heavy を通す bin check と filter 付き nextest だけ。
  全件 / test-full / dev・release ビルド / 実アプリ操作 / commit は行っていない。
