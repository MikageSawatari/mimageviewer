# 検索結果からの一覧順移動 (§1.682) と、検索中の場所ジャンプ (§1.688)

2026-11-11 第 3 版 (設計 = ClaudeCode Opus 5.5)。第 1 版は REJECT、第 2 版は ACCEPT WITH CHANGES で、指摘は反映済み (§7)。設計の独立レビュー = Codex GPT-6.1 Sol / xhigh、
実装 = Codex GPT-6.1 Sol / high、実装の独立レビュー = 別の Sol / xhigh。
コードの事実は 2026-11-11 の master (`2d2f8fd72`) を読んだもの。実行時の観測は無い (エージェントは製品を起動しない)。

- §1.682 (5ch >>704): Everything 検索結果から開いた本でも、一覧の表示順で Ctrl+Shift+↑/↓ (`FsListOrderPrev/Next`)
  で前後の本へ移れるようにする。
- §1.688 (5ch >>717、>>721 で「まず検索を終了して移動できる方向」と回答済み): 検索中でも、検索を終えて
  デスクトップ等へ場所ジャンプできるようにする。検索結果の履歴化 (保留 §1.689) とは分ける。

両方とも「検索結果の一覧」と「通常の移動」の境界を触るので、同じ文書で扱う。

## 1. 現状 (コード)

### 1.1 検索の所有と、閉じる・切り替える・取り消す経路

検索 surface は 3 つの owner がある (`TopLevelGridSurface::Search(Global | Favorite | Tag)`)。
`global_search` (Ctrl+G Metadata / Ctrl+R Everything、`SearchAccepted::{Metadata, Everything}`)、
`favsearch` (Ctrl+S)、`tag_view` (Ctrl+T)。いずれも `ViewerContextBundle` の field で、F12 fork の子には持ち越さない。

| 経路 | 入口 | 結果一覧 | 備考 |
| --- | --- | --- | --- |
| 通常 close (Esc・×) | `dismiss_*_with_origin` | origin を 1 回消費して元の場所へ戻す | |
| 別検索・smart へ切替 | `dismiss_*_without_restore` → origin 移譲 | 新しい surface へ | search-architecture §7.4 |
| 明示ジャンプ (右クリック「フォルダに移動」、grid の物理親) | `begin_context_jump_to_folder` → worker scan → `MainHistoryOperation::Direct(ExitTemporaryView)` | **採用成功の時点**で `dismiss_transparent_view_for_admitted_jump` が 3 owner をすべて `_without_restore` で閉じる (`src/app.rs` 約 25959・26038) | scan 失敗・snapshot 拒否・stale は何も閉じない (検索は残る) |
| 閲覧履歴・ブックマーク・評価一覧・コレクション・smart の install | 各 install | install 時に 3 owner を `_without_restore` で閉じる (成功時だけ) | 閲覧履歴・評価一覧は検索中 history が `Restore` になり、履歴に残らない |
| Everything の取得中 | 取消可能な main modal | 他の main 操作を受け付けない | 取消・× は常に受理 |
| Everything 本からの帰路 | `return_from_everything_child(Parent | Results)` | 保持 dataset を再投影 (IPC なし)、`return_anchor` を選択 | §1.664 の通常 close も Results |
| 遅着の検索完了 | Metadata/Everything Ready は active・run・context・items generation を照合。favsearch の poll は照合なし、tag は query/kind だけ | — | 3 owner が閉じられると pending も drop される |

### 1.2 §1.682 に関わる事実

- `capture_main_folder_list_order` (`src/app/list_order_navigation.rs` 52) と `list_order_position` (同 85) は
  `surface == Folder` を要求する。Everything の結果一覧 (Root) も、そこから開いた本 (PhysicalChild) も
  surface は `Search(Global)` のまま (`install_admitted_physical_surface_with_book_append`、`src/app.rs` 約 38306)。
  よって Everything の本では常に「一覧から開いた本で使えます」になる。
- Everything の行から本を開く入口は通常フォルダと同じ capture 呼び出し 5 か所 (`grid_physical_navigation` と
  `MainGridArchive` の 4 か所) を通る。今は surface の条件で何もしない。
- Everything の位置: `EverythingPosition::Root | PhysicalChild { logical_source, return_anchor, route }`
  (`src/global_search_ui/everything.rs` 111)。Root→PhysicalChild は `accept_everything_physical_child(path)` (519) だけで、
  成功採用の `commit_staged_search_navigation_effects` (`src/app.rs` 約 38349) から `advance_drilled_current_path` 経由で呼ばれる。
  **PhysicalChild のときは何も変えない**。
- 本の中の Ctrl+↑↓・Ctrl+PageUp/Down は Everything では拒否 (`SearchResultList` / `SearchSiblingUnsupported`)。
- `FolderNavMode::ListOrder` の着地は `load_folder_nav_target` / `start_physical_history_transition_*` の通常経路で、
  検索を閉じない。どちらも `capture_search_open_origin()` を捕捉し、成功採用で上の commit を呼ぶ。
- 結果一覧の順は `current_grid_order()` (詳細表示なら `details_order`、それ以外は `visible_indices`) で、
  Everything の rows も同じ。並べ替え・絞り込みの再投影は Root でだけ行う (`everything_dataset_for_reprojection`)。
- 採用済みの検索の同一性は `SearchAccepted::Everything { submitted: SubmittedEverything { query, run }, .. }` の `run`。
  `global_search.run_sequence` は送信のたびに進むが、Results 帰還では進まない。

### 1.3 §1.688 に関わる事実 (入口の棚卸し)

`location_navigation_blocked` (`src/app/gamepad_input.rs` 7658) は ★固定 → 検索 3 owner の順に拒否する。

| 入口 | 検索中の今の挙動 |
| --- | --- |
| ドライブ文字・現在ドライブのルート・ドライブ切替 (`apply_ring_drive_letter` / `apply_current_drive_root` / `apply_switch_drive_letter`) | 拒否 |
| 場所アクション (`apply_ring_location_action`: ドライブ一覧・閲覧履歴・本棚・デスクトップ・ピクチャ・ダウンロード・評価一覧★N) | 拒否 |
| 上の 2 行への経路: キー (`apply_location_navigation_key_action`)、リング・マウスジェスチャ・マウスボタン・ゲームパッドのリング (`apply_ring_action`) | 同じ helper を通るので拒否 |
| ゲームパッドの場所リスト (`open_gamepad_location_picker` 9398) | 開くこと自体を拒否 (「検索中は場所リストを開けません」) |
| ツールバーの 場所▼ メニュー (`ui_main.rs` 約 16074 `place_nav_enabled`) | 無効化 |
| お気に入りスロット・前後 (キー等、`apply_ring_favorite_slot` / `apply_favorite_cycle_nav`)、メニューバーのお気に入り、ゲームパッドのお気に入りリスト | 拒否しない。`Direct` が検索中 `Restore` に書き換わり、**検索を閉じずに** 擬似的な子として読み込む |
| ツールバーのお気に入り | 移動前に検索を **先に閉じる** (行き先が無効でも結果が消える) |
| アドレス欄・フォルダペイン | 拒否しない。検索を閉じずに擬似的な子として読み込む |

宛先ごとの下流:

| 宛先 | 下流 | 検索を閉じるか |
| --- | --- | --- |
| 実フォルダ (デスクトップ等・ドライブ・本棚) | `AddressBarNav::Direct` → `open_direct_navigation_target_at_stage` | 閉じない (擬似的な子) |
| ドライブ一覧 | `enter_drive_list_from_navigation` → `adopt_synthetic_surface(Direct(Navigation))` | 閉じない (検索の上にドライブ一覧が載る) |
| 閲覧履歴 | `enter_reading_history_with_entrance` | install で閉じる。history は Restore |
| 評価一覧 | `enter_rating_view` → `start_rating_navigation` → commit | commit で閉じる。history は Restore |

前例: grid の物理親 (`handle_grid_physical_parent_action`、`src/app/grid_parent.rs` 約 430) は、物理フォルダを
`begin_context_jump_to_folder` へ、ドライブ一覧を `adopt_synthetic_surface(Direct(ExitTemporaryView{None}))` へ渡す。
§1.688 はこの形をそのまま場所ジャンプへ広げる。

## 2. 決定

### 2.1 §1.688: 検索中の場所ジャンプは「検索を終える明示ジャンプ」にする

1. **入力の時点では行き先だけを決め、受付の判定は dispatch の時点で 1 回行う** (第 3 版、§7 第 2 版 P2)。
   - 入力側: `location_navigation_blocked()` を外す。★固定中の拒否だけは今の位置にも残す (fullscreen を閉じる前に通知するため)。
     それ以外は検索中でも行き先の解決へ進む。
   - キーの navigation 値はフレームの前半で作られ、同じフレームの検索バー描画 (一時バー・常設欄、`src/app/pinned_search.rs` 798) で
     Everything / Metadata / favsearch の送信・session 開始が起きた後、フレーム末尾で dispatch される
     (`src/app.rs` 約 100515・101050・101130、`src/app/update_phases.rs` 1239)。下流の `document_open_modal_admission_blocked` は
     Everything を含まず、`adopt_synthetic_surface` にも判定が無い。したがって検索中かどうか・取得中 modal かどうかは
     **dispatch の時点で** 決める。入力の時点の検索状態で mode を決めない。
2. **行き先を型で表し、対象の場所ジャンプは常に同じ navigation 値で運ぶ**。
   `enum LocationJumpTarget { Physical(PathBuf), DriveList, ReadingHistory, BooksRoot, Rating(u8) }`、
   `AddressBarNav::LocationJump(LocationJumpTarget)`。
   既存の各 helper (`apply_ring_drive_letter` 等、`quick_location_nav`) は行き先の解決 (存在確認・通知) までを今のまま行い、
   最後に共通の `fn location_jump_nav(&mut self, target, source) -> Option<AddressBarNav>` へ渡す。
   fullscreen を閉じる・`bump_input_seq`・repaint もこの helper に集める (今は各 helper に重複している)。
   評価一覧も同期の `enter_rating_view` 呼び出しをやめ、`Rating(n)` としてこの値で運ぶ。
3. **dispatch は 1 か所**。`AddressBarNav::LocationJump(target)` を受ける全 match
   (`update_phases.rs` の input/address・context・grid の 3 か所、`apply_fullscreen_close_nav_immediate` 等の即時 dispatcher)
   は同じ `fn dispatch_location_jump(&mut self, target)` を呼ぶ。中身:
   1. ★固定中 → 今と同じ通知で終わる。
   2. 主一覧の open / navigation が共有する Everything 取得中の target-context admission が拒否 → 何もしない
      (案内は既存の取得中表示に任せる)。
   3. 検索 3 owner のどれも active でない (`Plain`) → **今と同じ下流** (`Direct(path, ExplicitList)` を
      `open_direct_navigation_target_at_stage` へ / `enter_drive_list_from_navigation(None)` /
      `enter_reading_history_with_entrance(Menu)` / `open_books_root` / `enter_rating_view(n)`)。検索外の挙動は変えない。
   4. 検索のどれかが active (`ExitTemporaryView`) → 既存の前例に乗せる:
   - `Physical(path)`: `begin_context_jump_to_folder(JumpToFolderRequest { destination, selection: None, origin: None })`。
     `destination` はディレクトリなら `PhysicalDirectory`、ZIP/PDF/EPUB/変換書庫なら `ArchiveContainer`
     (分類は既存 helper の解決結果か拡張子分類で行い、UI スレッドに新しい同期 I/O を足さない)。
   - `BooksRoot`: 既存 `open_books_root` の作成処理の後、本棚のパスで上の `Physical` と同じ。
   - `DriveList`: `adopt_synthetic_surface(drive_list, Direct(ExitTemporaryView{None}), Some(None))` (grid 物理親と同じ)。
   - `ReadingHistory` / `Rating(n)`: 既存入口に `MainHistoryOperation` (または purpose) を渡せるようにし、
     `Direct(ExitTemporaryView{None})` で入る。install/commit が既に検索を閉じているので、変わるのは
     「検索中は history が Restore になる」点だけ (他の宛先と同じく履歴に残す)。
   - いずれも検索を閉じるのは **既存の採用成功の境界** (`dismiss_transparent_view_for_admitted_jump`、または各 install/commit)
     だけ。受付・scan 中・失敗・取消・stale では検索結果を残す。新しい pending・bool・二段処理は作らない。
4. **入口をすべてこの helper に通す**: キー・リング・マウスジェスチャ・マウスボタン・ゲームパッドのリング
   (すべて `apply_ring_action` / `apply_location_navigation_key_action` 経由)、ゲームパッドの場所リスト
   (開くことを許可し、確定 `confirm_gamepad_location_picker` の宛先も `LocationJumpTarget` + 同じ helper)、
   ツールバーの 場所▼ メニュー (検索中も有効にし、項目の確定を同じ helper へ)。
   - ゲームパッド場所リスト・場所▼ のブックマーク項目は `open_bookmark_browser` のまま (install が既に検索を閉じる)。
   - 場所▼ の「整理先」(`ui_main.rs` 約 16161) も実フォルダの宛先として同じ helper (`Physical`) へ通す。
   - 場所▼ の変更は有効条件と確定先の差し替えだけにする (§1.756 が同じファイルの 履歴▼ を触るため、周辺を整形しない)。
5. 検索中のジャンプ後の「戻る (←)」は、右クリック「フォルダに移動」と同じく検索前の場所へ戻る (ExitTemporaryView の既存契約)。
   検索結果は履歴に積まない (§1.689 は保留のまま)。

**初回実装の範囲外 (§9 でツールバーのお気に入り・ピン留め本を追加)**: お気に入り (キー・リング・メニューバー・ツールバー・ゲームパッドのお気に入りリスト)、
アドレス欄、フォルダペイン、メニューバーの「閲覧履歴」「評価一覧」「本棚」(`ui_main.rs` 約 8641・8740・8958。
今は検索中も有効で、閲覧履歴・評価一覧は検索を閉じて履歴に残さず、本棚は検索を閉じずに擬似的な子として開く)。これらは今も検索中に動く (拒否されていない) ので >>717 の不便には当たらず、
挙動を揃えると「検索中に擬似的な子として開く」リリース済みの挙動が変わる (CLAUDE.md「通常の使い方が不便になる変更は相談」)。
ただしツールバーのお気に入りは「先に検索を閉じる」ため行き先が無効だと結果が消える。この不揃いは利用者に報告し、
揃えるかどうかを別項目で相談する。

### 2.2 §1.682: Everything の結果一覧から開いた本でも一覧順で移動する

1. **列の所属 (scope) を 1 つの導出関数にする**。
   ```rust
   enum ListOrderScope { Folder, Everything { run: u64 } }   // run = accepted SubmittedEverything.run
   struct ListOrderSnapshot { owner: ViewerContextId, scope: ListOrderScope, entries: Arc<[ListOrderEntry]> }
   fn current_list_order_scope(&self) -> Option<ListOrderScope>
   ```
   `current_list_order_scope` は main context・★固定が無効のときだけ値を返す:
   - surface が `Folder` → `Folder` (今の条件そのまま)
   - surface が `Search(Global)` で `global_search.active` かつ `accepted` が Everything → `Everything { run }`
   - それ以外 (Metadata・favsearch・tag・smart・評価一覧など) → `None`
   capture と position は同じ関数を使う (1 つの意味に 1 つの書き手)。
2. **取得 (capture)**: 既存の 5 か所の呼び出しのまま。条件の `surface == Folder` を次に置き換え、scope を snapshot に入れる。
   Collection / Rating owner の除外はそのまま。列の中身は今と同じ (`current_grid_order()` 順の Folder / Zip / Pdf /
   ConvertibleArchive 行、path 重複なし)。
   - `Folder` scope: 今と同じ。
   - `Everything` scope: **Root で、かつ表示中の行が検索結果そのもの (`items_are_global_search_view`) のときだけ** 取る。
     並べ替え・★/タグ/ファセット/Ctrl+F 絞り込み後の表示順。PhysicalChild の中 (実フォルダ行の物理一覧) や、Root のまま
     アドレス欄等で dataset 外の物理一覧へ移った状態から本を開いたときは取らず、前の列を残す (§7 第 1 版 P1、第 2 版 P2)。
3. **位置 (position)**: `snapshot.scope == current_list_order_scope()` を足す。表示先 (MainWindow / Fullscreen)・
   detached でないことの条件は今のまま。今の本の求め方は scope で分ける:
   - `Folder`: 今と同じ (`effective_folder()` の完全一致 → Folder 行の配下)。
   - `Everything`: **採用済みの `PhysicalChild.logical_source` と完全一致する entry** の位置。Root・一致なしなら `None`。
     検索 session が「どの結果行を開いているか」を既に持っているので、物理パスの祖先推定をしない。Everything が
     `A` と `A\B` の両方を返した場合や、内側の書庫も結果行である場合でも、位置は開いた行に一意に決まる。
     Folder 行を開いてその中の本を開いた状態から押すと、その Folder 行の前後の結果行へ移る。
   - ただし位置を返す前に、**今表示している論理の場所がその行に属すること**を確かめる: `effective_folder()` がその行と同じ
     physical key、または行が Folder でその配下 (成分境界)。範囲外の入口 (アドレス欄等) で別の場所へ移った後は `logical_source`
     が旧行のまま残るので、この確認が無いと別の本を見ているのに旧行の次へ移ってしまう (§7 第 2 版 P2)。
   - 比較規則: Everything の entry・`logical_source`・採用された path の照合は、既存の dataset membership と同じ
     `crate::path_key::physical_path_spelling` の key で行う (`path_eq` ではない。I/O なし、§7 第 2 版 P3)。
   - 帰結: 検索を閉じた後・別の結果を採用した後 (run が変わる)・Ctrl+G の本・通常フォルダでは Everything の列は使えない。
     Everything の本の中では、通常フォルダで取った列は使えない。どちらも「一覧から開いた本で使えます」の通知。
   - 列は 1 つだけ持つ (scope ごとに持たない)。Everything から本を開くと、それまでの通常フォルダの列は置き換わる。
     状態を増やさないための割り切りで、次に通常フォルダから本を開けば取り直される。
4. **検索結果が更新されたときの順序**: 列は凍結 (通常フォルダと同じ)。
   - 結果一覧へ戻って並べ替え・絞り込みを変えても、次にその一覧から本を開くまでは前の順を使う。
     本の中に居る間は再投影が起きない (Root でだけ行う)。
   - 再検索は、**新しい結果が採用された時点で** accepted の run が変わり、古い列は使えなくなる。送信・取消・失敗だけでは
     accepted (旧 dataset と旧 run) が残るので、旧列も使える。source 切替も accepted が変わるまでは同じ (§7 P3)。
   - 追従・再取得の処理は作らない。移動のたびに Everything へ再問い合わせしない。
5. **移動先の行を型で運び、成功採用の境界で検索側の基準を揃える** (第 1 版のパス規則は撤回、§7 P1・P2)。
   - 一覧順の移動先が決まった時点 (`apply_folder_nav_result` の `FolderNavMode::ListOrder` で、snapshot の scope が
     `Everything { run }`) に、その要求へ **移動先の結果行** を持たせる:
     `SearchOpenOrigin::Everything { run, context, child: EverythingChildAdmission }`、
     `enum EverythingChildAdmission { Descend, ListOrderRow { dataset_run: u64, row: PathBuf } }`。
     既定は `Descend` (今の動き)。`ListOrderRow` を作るのはこの 1 か所だけ。
     **2 つの run は別物**: 外側の `run` は今と同じ `capture_search_open_origin()` の値 (現在の `run_sequence`)、
     内側の `dataset_run` は snapshot の scope の値 (accepted `SubmittedEverything.run`)。取消後は両者が異なる。
     「現在の `capture_search_open_origin()` に `ListOrderRow { dataset_run, row }` を付ける」形にする。
   - 載せ方: 要求ごとに既に運ばれている search origin (`load_folder_with_scan_claimed` の install closure、
     `PhysicalHistoryTransition.global_search_origin`、direct PDF の採用、変換書庫の継続) に、一覧順の移動の要求だけが
     `ListOrderRow` を渡せるよう、引数または要求の field で通す。App の新しい pending・bool・「次の採用で使う」field は作らない。
     どの関数に引数を足すかは実装者がコードで決め、全経路を列挙して報告する。設計レビューの確認では、実フォルダは
     install closure (hydration 開始前に検索側 commit)、ZIP/PDF/EPUB/変換書庫は staged 経路で、分類完了後に
     `PhysicalHistoryTransition` を作る **`ClassifiedOpenContinuation::Physical`** (`src/app.rs` 約 36323・30296) にも
     origin を再捕捉せず渡す必要がある。変換書庫・EPUB cache は元の `request.path` を検索側 commit へ渡す。
     direct PDF の採用と変換ダイアログの直接呼び出し (`src/ui_dialogs/archive_convert.rs` 1148) は一覧順の着地では通らないので `Descend` のまま。
   - 採用 (`commit_staged_search_navigation_effects` → `advance_drilled_current_path` → `accept_everything_physical_child`) で:
     - `Descend`: 今と同じ (Root なら行へ、PhysicalChild なら変えない)。**一覧順以外の移動の挙動は変えない**。
     - `ListOrderRow { dataset_run, row }`: origin が current (既存の run・context・active 照合)、accepted の run が `dataset_run`、
       採用された path が `row` と同じ physical key、`row` が dataset の行、のすべてを満たすときだけ
       `PhysicalChild { logical_source: row, return_anchor: row, route }` にする (`route` は今の値を保持)。
       満たさなければ何もしない (取消・失敗・stale では採用が起きないか、基準が変わらない)。
   - Folder 行から中の書庫へ着地する場合 (§1.444 §3.4 の 5-2): 一覧順の要求は Folder 行の list 採用 (path = 行) で基準を置き換え、
     続く書庫の open は通常の `Descend`。実装者はこの順序をコードとテストで確かめ、採用される path が行と一致しない経路が
     あれば止めて報告する。この着地の後の §1.664 の通常 close は、結果から同じ Folder 行を直接開いた場合と同じ条件
     (`effective_folder() == logical_source` を満たさないので結果へは戻らない。既存の挙動のまま)。
   - 結果へ戻る (Results)・§1.664 の通常 close・BS の親は置き換え後の基準を使う。結果へ戻ると移動後の本が選択される。
6. **入力と gate**: `FsListOrderPrev/Next` の既存 handler・native 動画・gate (modal・準備中 lock) は変えない。
   Ctrl+↑↓・Ctrl+PageUp/Down の検索中の拒否も変えない。新しいリング・ゲームパッド割り当ては足さない (§1.444 と同じ)。
7. **着地**: `FolderNavMode::ListOrder` の既存経路のまま (検索を閉じない)。設計レビューで、`apply_folder_nav_result` と
   `start_folder_nav` に `global_search.active` 中に ListOrder を拒否・別処理する分岐は無いと確認済み。
8. **範囲外**: Ctrl+G (Metadata の drill)・Ctrl+S (`nav_stack`)・タグビュー。別々の drill / nav_stack owner が要るため
   (Everything 計画 §7.1 の §1.664 と同じ判断)。今までどおり通知だけ。

### 2.3 複数ウィンドウ (F12)

- §1.682: `list_order_position` の既存条件 (main context・MainWindow/Fullscreen・detached でない) を変えない。
  snapshot は既存の bundle field のまま (中身に `scope` が増えるだけ)。fork の子は空、park/mount で一緒に動く分類も変えない。
- §1.688: 場所ジャンプは今も main grid / main fullscreen の入力。detached viewer の入力経路・述語・viewport は触らない。
- よって detached 述語 / viewport / registry の遷移には触れない見込み。実装でこれらに触れる必要が出たら、
  実装者は止めて報告する (CLAUDE.md の凍結ルール)。

## 3. 守ること (受入条件)

- §1.688: 行き先が存在しない・読めない・★固定の範囲外・stale のときは、検索結果・選択・入力欄を失わず通知する。
  採用に成功した時点でだけ検索を閉じる。採用後に遅れて届いた検索の完了 (Metadata streaming・Everything Ready・
  favsearch・tag) が移動先の一覧を上書きしない (閉じたときに pending が drop されることをテストで確かめる)。
  採用前に届いた検索の完了は、既存の明示ジャンプと同じ扱い (ジャンプが取り消されるなら検索は残る) で、
  どちらかが半端に混ざらない。
- §1.688: 検索外の場所ジャンプの挙動・履歴は変わらない。★固定中の拒否は変わらない。
- §1.682: 通常フォルダの一覧順移動・従来のフォルダ移動 (Ctrl+↑↓・兄弟) は変わらない。
  一覧順で移った後、結果へ戻ると移動後の本が選択される。§1.664 の通常 close も移動後の本で結果へ戻る。
  一覧順以外の検索中の移動 (本の中の移動・アドレス欄・フォルダペイン等) で、Everything の帰路の基準は変わらない。
- まれな失敗 (scan 中のディスク異常等) は既定の割り切り (ログ + 通知)。新しい回復処理を足さない。

## 4. 検討した単純化 (CLAUDE.md「設計の簡素化」)

- **§1.688 を「gate を外すだけ」にしない**: 外すと検索の上にドライブ一覧が載る・検索が擬似的な子として残る
  (下流が検索を閉じない)。採らない。
- **「検索を閉じて元の場所へ戻してから再ジャンプ」の二段処理にしない**: 中間の復元 worker と組み合わせが増え、
  失敗時に結果が先に消える。採らない。既存の ExitTemporaryView の成功時 dismiss に一本化する。
- **新しい pending を足さない**: 既存の明示ジャンプの scan・`adopt_synthetic_surface`・評価一覧/閲覧履歴の採用に乗せる。
- **§1.682 の位置は index で持たない** (§1.444 第 3 版と同じ)。Everything では検索 session の `logical_source` から引く。
- **移動先の行は型で運ぶ** (第 2 版): 第 1 版の「採用された path と dataset のパス関係から推定する」案は、親子の結果行で位置と帰路が
  決定的にずれ、一覧順以外の移動の挙動も変えるので撤回した (§7)。運ぶのは移動先の行 1 つだけで、既に要求ごとに運ばれている
  search origin に載せる。新しい App 状態は作らない。
- **列を scope ごとに複数持たない**: 1 つに置き換えるだけにした。Everything から本を開くと通常フォルダの列は失われる。
- **検索結果の更新に列を追従させない**: 凍結。新しい結果の採用で run が変わり、自然に使えなくなる。
- **Everything の列は Root でだけ取る**: 子の物理一覧からも取ると、位置の求め方が 2 通り要る (結果行と物理パス)。
- **お気に入り・アドレス欄・フォルダペイン・メニューバーの入口まで揃えない**: リリース済みの挙動が変わるので相談対象 (§2.1 範囲外)。

## 5. テスト (handler-level と状態遷移)

§1.688 (各ケースで Metadata / Everything / favsearch / tag の 4 種):
- キー・リング (`apply_ring_action`)・マウスボタン・ゲームパッドのリング・ゲームパッド場所リスト・場所▼ の各入口から、
  デスクトップ (実フォルダ)・ドライブ文字・ドライブ一覧・閲覧履歴・本棚・評価一覧へ移ると、採用後に検索が閉じ、
  移動先が表示され、← で検索前の場所へ戻れる (履歴が記録される)。入口が違っても同じ結果になる。
- 存在しない / scan 失敗の行き先: 検索 active・結果の行・選択が残り、通知が出る。
- ジャンプの scan 中に検索の完了が届く場合と、採用後に届く場合。採用後は一覧を上書きしない。
- 場所キーの navigation 値を作った同じフレームで Everything の送信が始まると、ジャンプは実行されない (dispatch 時の判定)。検索なしの状態から常設欄で Everything / Metadata / favsearch を開始した同じフレームでは、ExitTemporaryView 側の規則になる (または取得中なら実行されない)。
- 場所▼ の「整理先」も同じ規則。範囲外の入口 (お気に入り・アドレス欄・フォルダペイン・メニューバー) は今の挙動のまま (回帰)。
- ★固定中は今の拒否のまま。検索外では今と同じ `AddressBarNav` (回帰)。
- fullscreen (検索結果から開いた画像・本) からの場所ジャンプ: fullscreen を閉じて同じ規則。

§1.682:
- Everything Root から本を開くと scope=Everything{run} の列が取れる。PhysicalChild の物理一覧から開いても列は変わらない。並べ替え (名前・日付の昇降順)・詳細表示・
  ★/タグ/ファセット/Ctrl+F 絞り込みの順と一致し、画像・動画行は入らない。複数ドライブの本と単体ファイルの混在。
- Metadata・favsearch・tag・評価一覧などでは取らず、既存の列も変えない (`list_order_capture_excluded_surfaces_*` を
  `Search(Global)` の Everything / Metadata に分けて更新)。
- PhysicalChild の本で位置が出て、前後の行へ移れる (ZIP・PDF・EPUB・画像フォルダ本・変換書庫・実フォルダ)。両端の通知。
- 移動後に `logical_source` / `return_anchor` が移動先へ置き換わり、Results 帰還で移動先が選択される。§1.664 の
  通常 close も結果へ戻る。BS の親も移動先基準。ZIP・PDF (cold / warm)・EPUB・変換書庫・実フォルダ・画像フォルダ本の各着地経路。
- `A` と `A\B` が両方行の場合: 一覧順で `A` → `A\B` へ移ると基準が `A\B` になり、次の押下は `A\B` の次の行へ進む。
  `A` の中の物理移動で `A\B` に入った場合は基準が `A` のまま。Folder 行から中の書庫への着地。内側の書庫自体も結果行の場合。
- 一覧順以外の移動 (本の中の移動・アドレス欄やフォルダペインで別の結果行へ) では基準が変わらない (回帰)。
- 新しい結果の採用後・検索 close 後・通常フォルダでは Everything の列で位置が出ない。旧 accepted 表示中の再送信・取消・失敗・
  source 切替では旧列が使える。逆に Everything の本では通常フォルダの列が使えない。
- 移動の取消・失敗で基準が変わらない。F12 linked / detached・★固定で使えない (既存条件)。
- 通常フォルダの一覧順移動の既存テストがすべて通る。

## 6. 文書

- [一覧順ナビゲーションの設計](list-order-book-navigation-plan.md): §3.3・§3.4・§5・§6 に scope と Everything の扱いを追記。
- [Everything 検索の設計](everything-search-plan.md) §7.1 と [検索アーキテクチャ](search-architecture.md) §1.2.1・§7.4:
  PhysicalChild の基準の置き換え規則、場所ジャンプの成功時終了。
- [keymap-spec](keymap-spec.md): 静止画・native 動画の一覧順の説明 (約 1081・1360) と、場所ジャンプの検索中の扱い。
- [key-customization-impl-plan](key-customization-impl-plan.md) 約 69、[architecture-overview](architecture-overview.md) 約 34 の一覧順の説明。
- マニュアル: `fullscreen.html#list-order-navigation`・`search.html` (Everything 結果の本・検索中の場所ジャンプ)・
  `gamepad.html` (場所リスト)・`shortcuts.html`・他に「検索中は場所ジャンプを使用できません」等の記述があれば更新。
  製品ページ `index.html` の「一覧順で本を巡回」カード (150 字以内、`python scripts/check_site_cards.py`)。
- `docs/next-release-backlog.md` §1.682・§1.688 の状態。

## 7. 設計レビューへの対応

### 第 1 版 (Codex GPT-6.1 Sol / xhigh、REJECT、2026-11-11)

- P1 (親子の結果行で位置が祖先へずれる): Everything scope の位置を `logical_source` の完全一致で引き、capture を Root だけにした (§2.2-2・3)。
- P2 (パス規則では帰路と既存挙動を守れない): パス規則を撤回し、移動先の行を `EverythingChildAdmission::ListOrderRow` で要求に載せ、
  成功採用で一致したときだけ置き換える。一覧順以外の移動は不変 (§2.2-5)。
- P2 (同一フレームの Everything 送信を追い越す): dispatch 時に取得中 admission を再判定する (§2.1-1)。
- P2 (入口の棚卸し不足): 場所▼ の「整理先」を含めた。メニューバーの閲覧履歴・評価一覧・本棚は検索中も動いている入口なので、
  お気に入り等と同じく範囲外として今の挙動を記録した (§2.1 範囲外)。
- P3 (run の説明): 新しい結果の採用で無効になる、と書き直した (§2.2-4)。
- P3 (文書): keymap-spec・key-customization-impl-plan・architecture-overview を追加した (§6)。
- レビューで確認された事項: ListOrder の開始・採用に検索 active の拒否は無い。検索終了時の pending 破棄は既存処理で成立する。
  detached 述語・viewport・registry の変更は不要。新しい同期 I/O は不要。

### 第 2 版 (同じレビュー担当の再レビュー、ACCEPT WITH CHANGES、2026-11-11)

- P2 (Root だけでは表示中の一覧が検索結果と限らない): capture に `items_are_global_search_view` を足し、position の前に
  今の論理の場所が行に属することを確かめる (§2.2-2・3)。
- P2 (Plain の場所ジャンプが dispatch 時の判定を通らない): 対象の場所ジャンプは検索の有無によらず `AddressBarNav::LocationJump`
  で運び、mode・modal・★固定を dispatch 時に判定する (§2.1-1〜3)。
- P3 (2 つの run): `run` (現在の run_sequence) と `dataset_run` (accepted の run) を分けて書いた (§2.2-5)。
- P3 (比較規則): physical key (`physical_path_spelling`) に揃えた (§2.2-3)。
- 輸送経路の確認結果 (実フォルダ・staged・`ClassifiedOpenContinuation::Physical`・変換書庫/EPUB cache・direct PDF) を §2.2-5 に記録した。
- 範囲外の整理、detached 述語・viewport・registry を変更しない点は妥当と確認された。

## 8. 実装記録 (2026-11-11、feat-682-search-nav)

初回実装は `3621aefec` (設計担当がコミット)。§8.1〜8.4 はその実装・検証時の記録で、
§8.5 は独立レビューへの追補修正 (`e85c702ef`)、§8.6 は設計担当の受入、§8.7 は §9 の追補実装 (`4a87140f3`)、
§8.8 は §10 の追補実装 (`ed6429060`)、§8.9 はその全件テスト失敗への追補修正 (未コミット)。
実装担当は製品バイナリの起動・実機観測・コミットを行っていない。
設計の前提を source で照合し、既存の capture 入口、成功採用境界、Folder 行の採用後に内部書庫を開く順序を維持した。
App の新しい pending / bool / 次回採用フラグ、detached 述語・viewport・registry 遷移の変更、新しい同期 I/O はない。

### 8.1 変更ファイルと実装

- `src/app/list_order_navigation.rs`: `ListOrderScope`、snapshot の scope、capture / position 共通の
  `current_list_order_scope`。Everything は accepted run、Root と結果 items のときだけ capture し、
  position は logical_source の physical key 完全一致と現在の論理場所の所属を照合する。
- `src/global_search_ui/everything.rs` / `src/global_search_ui.rs`: `EverythingChildAdmission` を
  `SearchOpenOrigin::Everything` に追加。既定は `Descend`。current origin / accepted dataset run /
  採用 path と行の physical key / dataset membership が揃った成功採用だけ `ListOrderRow` の基準を採用する。
  route は保持し、通常の Descend は既存どおり Root からだけ進む。
- `src/app.rs`: `apply_folder_nav_result` の Everything scope の ListOrder だけが `ListOrderRow` を作る。
  reading history / rating の既存入口は従来の history を選ぶ wrapper を残し、場所ジャンプだけ
  `ExitTemporaryView` を渡せる共通 tail にした。本棚作成も既存処理を共通 helper に抽出した。
- `src/app/location_jump.rs` / `src/app/gamepad_input.rs` / `src/app/update_phases.rs` / `src/ui_main.rs`:
  `LocationJumpTarget` と `AddressBarNav::LocationJump`、共通入力 helper / dispatch。
  キー・リング・ジェスチャ・マウスボタン・ゲームパッドのリング・場所リスト・場所▼ (整理先を含む) を通す。
  場所リストと場所▼は検索中も利用可能。お気に入り・アドレス欄・フォルダペイン・メニューバーは変更しない。
- `src/app/list_order_tests.rs` / `src/app/list_order_everything_tests.rs` /
  `src/global_search_ui/everything/tests.rs` / `src/app/location_jump.rs` / `src/app/gamepad_input.rs`: 回帰テスト。
  `src/tag_view.rs` は遅着完了の破棄を検査する `cfg(test)` 限定の pending constructor だけを追加。
  `src/app/tests/section1328_session.rs` は場所入力の既存テストの候補型を追従しただけ。
- `src/app/list_order_worker_tests.rs` / `src/app/native_video.rs` / `src/app/viewer_context_registry.rs` /
  `src/ui_fullscreen.rs`: 既存の Folder snapshot fixture に scope を明示するだけ。
  `src/app/startup_ops.rs`: 既存の分類待ち fixture に通常 origin (`None`) を明示するだけ。
- §6 の設計文書・キー仕様・構造概観、manual 4 ページ、製品紹介カード、backlog §1.682 / §1.688 を更新。

### 8.2 ListOrderRow の全輸送経路

起点はすべて `apply_folder_nav_result`。現在の `capture_search_open_origin()` の child に、
snapshot の accepted run と宛先の行を付ける。外側 run_sequence を snapshot run で置き換えない。

1. 実フォルダ・画像フォルダ本:
   `load_folder_nav_target_with_search_origin` → `load_folder_with_scan_owned_with_search_origin` →
   `load_folder_with_scan_claimed_with_search_origin` → install closure →
   `commit_staged_search_navigation_effects`。hydration 開始前に検索側も commit する。
2. ZIP / CBZ・PDF (cold / warm):
   `start_physical_history_transition_with_dfs_and_search_origin` →
   `start_physical_history_transition_classified_with_search_origin` → `admit_physical_history_phase` →
   `PhysicalHistoryTransition.global_search_origin` → `commit_physical_history_transition` →同じ commit。
   一覧順の PDF は staged へ入り、既存の direct PDF 即時採用経路は変更しない。
3. EPUB・変換書庫の非同期分類:
   上記の start → `ClassifiedOpenContinuation::Physical.search_origin` → 分類完了後の
   `start_physical_history_transition_classified_with_search_origin` → 上記 staged request。
   分類完了時に origin を再捕捉しない。
4. 変換確認・パスワード・変換完了・EPUB cache:
   既存 request 自体が継続を所有し、`commit_converted_physical_history_transition` 等から
   `commit_physical_history_transition` に戻る。検索へ渡すのは元の `request.path`。
   backing ZIP/PDF は検索行の identity に使わない。
5. Folder 行から内部書庫:
   1 の Folder 行採用で基準を更新 → 既存 fullscreen reopen →
   `find_fullscreen_nav_target_filtered` →通常の `load_folder_nav_target` (新規捕捉は `Descend`)。
   内部書庫自身が結果行にあっても、Folder 行の基準を維持する。

成功採用は `commit_staged_search_navigation_effects` →
`advance_drilled_current_path_with_admission` → `accept_everything_physical_child` の一つの経路。
通常呼出しの `advance_drilled_current_path` と通常 load の wrapper は `Descend` を維持する。
App に要求外の輸送 field は増やしていない。

### 8.3 LocationJump の dispatch 箇所

- `src/app/update_phases.rs::post_render_main_navigation`: input / address、context menu の open 結果、grid の 3 match。
- `src/app.rs::apply_fullscreen_close_nav_immediate`: 即時 dispatcher。

上の 4 か所すべてが `dispatch_location_jump` を呼ぶ。入力側では宛先のみを運び、
dispatch 時に ★固定・対象 context の Everything 取得中・既存 document modal を照合して、
現在の検索 owner から Plain / ExitTemporaryView を選ぶ。
Physical は既存 jump scan、DriveList は synthetic adoption、ReadingHistory / Rating は既存の install / commit へ渡し、
いずれも成功前に検索を dismiss しない。検索外は従来の各下流入口を使う。

### 8.4 検証

追加テストは 32 件 (ループ内で複数の検索 owner・着地を検査):

- `app/list_order_everything_tests.rs` 12 件: Root/items capture、名前・日付・詳細の昇降順、
  ★/タグ/facet/Ctrl+F、複数ドライブと media 除外、logical_source と現在場所の physical key、
  accepted run / 再送信 / 取消 / source 切替、origin と dataset の 2 run、
  8 着地経路の origin 輸送と Results の選択、通常 close、Parent、Folder→内部書庫と次の押下、取消・失敗・stale。
- `global_search_ui/everything/tests.rs` 2 件:
  `list_order_failed_query_preserves_the_accepted_run_snapshot_and_physical_child` と
  `list_order_row_admission_uses_physical_identity_and_preserves_the_original_route`。
- `app/location_jump.rs` 15 件: 4 検索 owner の実フォルダ scan 成功/失敗、DriveList / ReadingHistory / BooksRoot /
  Rating の採用と検索前履歴、Everything Root / PhysicalChild、Plain 候補後の Everything 取得開始・Metadata/favsearch 開始、
  producer 破棄と遅着送信、同一検索の行更新と新しい検索への切替の採用判定、★固定の再照合、CBZ、input / address / grid の dispatcher、
  5 宛先の Plain 回帰。Everything fixture は実際に dataset を採用し、accepted 型と元の origin を照合する。
- `app/gamepad_input.rs` 2 件: キー・リング・マウスボタン・ゲームパッドリング・場所リストの typed transport と、
  Plain transport / 範囲外のお気に入り Direct の回帰。
- `ui_main.rs` 1 件: 4 検索 owner で実際の headless toolbar を描画し、場所▼ → 整理先 → 宛先のクリックから
  typed candidate と非同期受付までを検査する。検索を受付時に閉じないことも照合する。

既存テストの変更理由:

- `list_order_capture_excluded_surfaces_preserve_the_previous_snapshot` は
  `list_order_capture_excluded_surfaces_and_metadata_preserve_the_previous_snapshot` に改名し、active Metadata を明示。
  Everything の意図した例外を新しい capture テストへ分けた。従来の除外 surface と保持の assert は残す。
- 既存 Everything テストの `accept_everything_physical_child` 呼出しは `Descend` を明示し、origin freshness は借用へ追従しただけ。
  期待する状態や assert は変えていない。
- 既存 snapshot / 分類待ち fixture の `scope: Folder` / `search_origin: None` は、新しい型の必須 field の指定。
  native / detached / registry を含め、既存の期待値を緩めたテストはない。
- `file_organize_destinations_location_menu_navigation_and_snapshot` と
  `file_organize_destinations_many_location_entries_scroll_and_select_last` は整理先の候補型だけを
  `Direct` から `LocationJump(Physical)` へ変更。Plain も dispatch で mode を決める意図した仕様変更への追従で、
  宛先 path・snapshot・menu close・スクロールの assert は残した。履歴・お気に入り・アドレス欄の期待値は変えない。
- `section1328_session` の場所キー入力の候補だけを `ReadingHistory(Menu)` から
  `LocationJump(ReadingHistory)` へ変更。4 つの入力 lane の採用・選択・履歴の検査と、親へ戻る場合の旧候補型は維持する。

独立 read-only 実装レビュー (Sol / xhigh) は製品の構造を ACCEPT。
レビューで見つかったテスト fixture の Everything 未採用と Plain→非modal検索の未収録は補完し、focused 再レビューで解消済み。
検証対象は本節の未コミット差分、Windows / MSVC / 通常 feature set。
Cargo 実行では `MSBUILDDISABLENODEREUSE=1` を使用。最初の直接 nextest は `CARGO_BUILD_JOBS=1`、
関連一式・最後の単独再検証・確認用 build は `CARGO_BUILD_JOBS=4`。
full suite は設計担当の PC 全体 lock 下で行うため、この実装セッションでは実行しない。

初回の場所ジャンプ 15 件は 3 成功 / 12 失敗。新規 fixture の履歴参照が旧 flat field を見ていたため、
A/B の正規 owner を読む `folder_history_back_entry()` へ修正した。★固定は元から検索を閉じるので固定直後を拒否前の基準にした。
同一 query の行更新は path を所有する明示ジャンプを継続できる既存契約であり、items generation の変更だけで拒否されるという
新規テストの前提を訂正した。新しい検索への切替は既存の退役入口と保存した遅着 scan で検査する。
Everything の producer 破棄は、別 query に替えてジャンプを無効化せず、同じ採用 query のまま pending を開始して検査する。
これらはテストだけの修正で、製品の adoption / history / Snapshot 規則は変更していない。

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --lib --tests` | 本体 lib / lib tests の型検査は通過。対象外の既存 `tests/catalog_integration.rs` の `CatalogDb::save` 4 呼出しが `CoverIdentity` 引数未指定で E0061。`src/catalog.rs` と当該テストは HEAD と同じで、今回変更していない。コマンド全体は失敗 (shell exit 1)。初回の新 variant の match / fixture 必須 field の指定漏れは修正済み |
| `cargo check -p mimageviewer --lib` | 成功、3m46s。`target/search-nav-lib-check.log` |
| `.\scripts\test-focused.ps1 -PlanOnly` | broad/shared 入力を検知して exit 2、テスト未実行。案内された full gate は実行していない |
| `.\scripts\test-focused.ps1 -Path src/app/location_jump.rs` | 2 回ともテスト開始前に `turbojpeg-sys` CMake/MSBuild で失敗 (0 件実行)。native の原因は未確定。ログは `target/test-focused/runs/20261111T053346910Z-51e8f29b/` と `20261111T054149346Z-de596bd3/` |
| `cmake --build target/debug/build/turbojpeg-sys-f4a0b3122eba9190/out/build --target install --config Debug --parallel 1 -- /verbosity:normal` | 同じ workspace の native 生成物の準備は成功 (exit 0、0 エラー、4m07s)。ただし成功時の ZERO_CHECK は up-to-date で、並列数を原因とは確定していない。`target/search-nav-turbojpeg-build.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(app::location_jump::)' --no-fail-fast` | native 段階を通過、初回 test compile 23m15s。15 実行 / 3 成功 / 12 失敗 (上記新規 fixture の誤り)。`target/search-nav-location-nextest.log`、nextest exit 100 |
| `cargo nextest run -p mimageviewer --lib -E 'test(list_order) \| test(everything) \| test(location) \| test(gamepad) \| test(jump) \| test(section1328)' --no-fail-fast` | 717 実行 / 716 成功 (slow 3) / 1 失敗、180.044s、compile 4m34s。残る失敗は新規テストが Everything Root の `current_folder` を合成パスと決め付けたもの。`target/search-nav-focused-nextest.log`、nextest exit 100 |
| `cargo nextest run -p mimageviewer --lib -E 'test(location_jump_same_query_refresh_adopts_but_new_query_rejects_old_scan)' --no-fail-fast` | 実際の受付前の folder を保存し、拒否後の不変を検査するようそのテスト内だけを修正。1 実行 / 1 成功、2.281s、compile 2m31s、exit 0。`target/search-nav-fixture-nextest.log` |
| `cargo fmt` (workspace、引数なし) / `cargo fmt --check` | 実行済み、成功。最後のソース変更後にも workspace 全体へ適用 |
| `python scripts/check_ui_glyphs.py` | 成功、危険 glyph 0 件 |
| `python scripts/check_site_cards.py` | 成功、53 cards / 上限超過 0 件 |
| `git diff --check` | 成功 |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 最終実行は exit 0 / DONE。本体 10m06s、付属 4 executable も生成し、FFmpeg / VCRT / EffeTune 配置と最終 PE gate `runtime=4 pe=5` が成功。通常 profile / 通常 feature set、portable 無効。実行条件は下記。取得した出力は `target/search-nav-build-dev-final.log` (大量の警告には tool 側の省略を含む)、PE report は `target/vcrt-pe-reports/dev-runtime.json`。製品は起動していない |

最終の関連検証は **717 個の異なるテストで成功** (有効な 716 件 + 修正後の単独 1 件を再利用)。
最後の変更は当該 1 テスト内の受付前 folder 保存と不変 assert だけで、製品・共通 fixture・他の 716 件は不変のため、
build/test policy に従い一式を重複実行していない。直接 nextest 等のログは `.\target\search-nav-run.py` で保存し、
それ以外の結果は当セッションのコマンド出力にも残した。これらは非対話検査で、
製品バイナリの起動やユーザーの実設定による実機検証の代替ではない。
native 調査は生成物 / 実行条件だけに限定し、依存ソース・build logic・個人設定は変更せず、他セッションの process も停止していない。

確認用 build の初回も、新しい `dev-runtime` 用 `turbojpeg-sys` の CMake/MSBuild Checking Build System で失敗した。
`C1041` / `MSB3191` の具体的診断はなく、原因は未確定。
Cargo が生成した同じ native build directory に対して
`cmake --build target/dev-runtime/build/turbojpeg-sys-ad9ab6b47bbc75e3/out/build --target install --config RelWithDebInfo --parallel 1 -- /verbosity:normal`
を実行し、成功 (exit 0、0 エラー、5m00.75s、`target/search-nav-turbojpeg-runtime.log`) を確認した。
これは同じ `turbojpeg-sys 1.2.0` の RelWithDebInfo / MT 生成物であり、先の Debug / MTd 生成物は流用していない。

その後の確認用 build では、同依存が提供する explicit / static 指定をプロセス環境変数で使用した。
`out/lib/turbojpeg-static.lib` を本 worktree 内の `target/turbojpeg-runtime-explicit/lib/turbojpeg.lib` にコピーし、
SHA256 が一致することを照合した。指定は次のとおり (すべて今回の PowerShell process 内だけ):

```powershell
$env:X86_64_PC_WINDOWS_MSVC_TURBOJPEG_SOURCE = 'explicit'
$env:X86_64_PC_WINDOWS_MSVC_TURBOJPEG_STATIC = '1'
$env:X86_64_PC_WINDOWS_MSVC_TURBOJPEG_LIB_DIR = 'G:\home\mimageviewer-682\target\turbojpeg-runtime-explicit\lib'
$env:X86_64_PC_WINDOWS_MSVC_TURBOJPEG_INCLUDE_DIR = 'G:\home\mimageviewer-682\target\dev-runtime\build\turbojpeg-sys-ad9ab6b47bbc75e3\out\include'
$env:CXXFLAGS_x86_64_pc_windows_msvc = '/IG:\home\mimageviewer-682\target\dev-runtime\build\turbojpeg-sys-ad9ab6b47bbc75e3\out\include'
$env:MSBUILDDISABLENODEREUSE = '1'
$env:CARGO_BUILD_JOBS = '4'
.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0
```

依存の build script 出力で `cargo:rustc-link-lib=static=turbojpeg` と上記の library / include directory を確認した。
最初の explicit 指定では libraw の `jpeglib.h` 探索が C1083 で失敗した
(`target/search-nav-build-dev-explicit-header-failure.log`)。
ヘッダーは存在し、依存の `fs::canonicalize` が `\\?\G:\...` 表記を下流へ渡していた。
同じ MSVC `cl.exe /nologo /EP` と `__has_include(<jpeglib.h>)` だけの非実行 probe で、
通常表記の `/IG:\...\include` は exit 0、`/I\\?\G:\...\include` は exit 2 になる差を確認した。
そこで上記の target 固有 `CXXFLAGS` に通常表記の include を追加した。製品・依存・build script のソース修正はない。
`-PreserveRuntime` は稼働中の開発用 runtime を停止しないため、待機時間 0 は所有者を確認できない他セッションの
MSBuild process を止めずに進めるための既存オプションである。通常 feature set / portable 無効は変えていない。

確認用成果物は `target/dev-runtime/mimageviewer-core.exe` と同 directory の remote / EPUB PDF / MCP /
music-video companion。最終 build の後に 5 ファイルの存在・更新日時を照合した。
利用者の手動確認は、インストール版・トレイ常駐版を閉じてから
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
既定では通常の `%APPDATA%\mimageviewer` を使用し、実設定・データを更新し得る。single-instance mutex も共用する。
確認対象は Everything の一覧順移動と移動先行への Results 復帰、Folder 行から内部書庫へ進む場合の基準、
各検索中の場所ジャンプの成功時終了 / 失敗時保持と、検索外の同等操作。
実機結果・full suite・コミットは設計担当へ引き継ぐ。

### 8.5 独立レビュー ACCEPT WITH CHANGES への追補 (e85c702ef でコミット)

利用者が伝えた別の Sol / xhigh 独立レビューの P2 / P3 に対応した。
変更ファイルは `src/app/location_jump.rs`、`src/app/list_order_everything_tests.rs`、本書だけ。

**P2: 候補生成後に開いた一般 modal の再照合**

- root のフレーム順序は場所キー等の候補生成 → `handle_delete_key` → `post_render_main_navigation`。
  削除確認は後者の handler が `show_delete_confirm` と削除対象を設定して開くが、従来の場所 dispatch は
  Everything 取得中と document-open modal しか拒否せず、旧 items を対象にした確認を残して別一覧を採用できた。
- `dispatch_location_jump` の Everything 専用判定を既存 `main_chrome_pointer_blocked()` に置き換えた。
  共通の `modal_dialog_block_reason_including_menus(false)` から導く既存判定を使い、メニューを除外する。
  Everything 取得中もこの共通判定に含まれる。★固定と document-open admission は維持し、
  Plain / ExitTemporaryView の分岐前に全 4 dispatcher で同じ判定を適用する。新しい modal 分類は作らない。
- Plain の旧下流も source で照合した。例えば `enter_drive_list_from_navigation` は document-open modal だけを見て
  `adopt_synthetic_surface` へ進み、一般の削除確認を再検査しなかった。場所▼等の通常入力は共通 chrome gate で止まる一方、
  生成済み候補と同じフレームで削除確認が開く競合は通過し得た。今回の Plain の変更は、この競合も拒否する修正である。
  Plain の通常操作・メニュー操作は維持し、同 frame の Delete 確認後の未 dispatch 候補を新たに拒否する。
- 新規 3 テスト: `location_jump_dispatch_rechecks_delete_confirmation_for_each_search`、
  `location_jump_plain_dispatch_rechecks_delete_confirmation`、`location_jump_dispatch_excludes_menus_from_the_modal_gate`。
  最初の 2 件は実際の Delete キー event と `handle_delete_key` を使う。4 検索 owner / Plain、5 宛先、
  input / address / grid / immediate の 4 dispatch lane について、検索 owner・surface・復元先・items generation・
  選択/チェック・履歴・削除確認対象の不変と、後続 open の未開始を照合する。
  メニューだけの場合は拒否しないことを別途検査する。

**P3: 変換確認・パスワード待ちでの ListOrderRow 輸送**

- 新規 2 テスト: `list_order_everything_conversion_confirmation_resumes_original_owner_or_cancels` と
  `list_order_everything_pdf_password_wait_resumes_original_owner_or_cancels`。各々が再開 / 取消を検査する。
- 変換確認は未キャッシュの実 7z と Ask 設定から実際の確認 phase へ進む。既存 Confirm handler で実変換 worker を開始し、
  headless `show_archive_convert_dialog` の `StagedHistory(request_id)` 完了 routing と ZIP preflight を通して採用する。
- PDF は既存 `TestPdfEnumeratePool` で外部 process の応答だけを模擬する。実 enqueue に `PasswordRequired` を返して
  既存 preflight からパスワード待ちを生成し、実際の retry / cancel handler を使う。再試行の enqueue へ成功応答を返す。
  dialog / PhysicalHistoryPhase / commit をテストから直接設定しない。
- 待機・再開中は同じ request id、現在の origin run と旧 accepted dataset run、ListOrderRow、元 source、凍結列を保持する。
  成功時は移動先の logical_source / return_anchor を採用し、Results 帰還でその行を選択する。
  取消時は旧基準・表示 items・fullscreen・凍結列を保持する。既存の cache 着地 / preflight テストも残した。

検証コマンドと結果はこの追補差分に対するもの。製品起動・full suite・コミットは行わず、
確認用 build は通常 feature set / portable 無効とする。Cargo の環境は `MSBUILDDISABLENODEREUSE=1`、
`CARGO_BUILD_JOBS=4`。確認用 build の native 指定は §8.4 の同じプロセス環境変数を使う。

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(location_jump_dispatch_rechecks_delete_confirmation_for_each_search) \| test(location_jump_plain_dispatch_rechecks_delete_confirmation) \| test(location_jump_dispatch_excludes_menus_from_the_modal_gate) \| test(list_order_everything_conversion_confirmation_resumes_original_owner_or_cancels) \| test(list_order_everything_pdf_password_wait_resumes_original_owner_or_cancels)' --no-fail-fast` | exit 0、5 実行 / 5 成功、1.198s (compile 2m24s)、17,255 skipped。`target/search-nav-review-new-tests.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(list_order) \| test(everything) \| test(location) \| test(gamepad) \| test(jump) \| test(section1328)' --no-fail-fast` | exit 0、722 実行 / 722 成功、72.996s (compile 1.10s)、16,538 skipped。`target/search-nav-review-focused.log` |
| `cargo fmt` (workspace 全体、引数なし) / `cargo fmt --check` | 成功、exit 0 |
| `git -c core.safecrlf=false diff --check` | 成功、exit 0 |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0 / DONE。本体 1m44s、remote 0.47s、EPUB worker 0.50s、MCP 0.36s、music-video editor 3.94s。5 executable の存在を照合し、最終 PE gate `runtime=4 pe=5` が成功。§8.4 の native 環境変数を同じ値で指定。進捗抜粋は `target/search-nav-review-build.log`、PE report は `target/vcrt-pe-reports/dev-runtime.json` |

追補差分の別 context の read-only 独立レビュー (Sol / xhigh) は **ACCEPT、追加指摘なし**。
製品修正は共通 modal 判定の置換 1 行だけで、ListOrderRow の輸送コードには修正がなかった。
今回追加した 5 件を含む関連 722 件の成功を記録する。full suite・実機観測・コミットは指定どおり未実施。
確認用本体は `target/dev-runtime/mimageviewer-core.exe` を更新済み。利用者が起動する場合の
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`、通常 `%APPDATA%\mimageviewer` の
実設定・データ更新の可能性、インストール版・トレイ常駐版を先に閉じる注意は §8.4 と同じ。

### 8.6 受入 (設計担当 ClaudeCode、2026-11-11)

- 実装の独立レビュー (Codex GPT-6.1 Sol / xhigh、実装担当とは別セッション): 初回 ACCEPT WITH CHANGES (P2: dispatch 時の共通モーダル再確認、
  P3: 変換確認・パスワード待ちの輸送テスト) → `e85c702ef` で対応 → 再レビュー ACCEPT (追加指摘なし)。
- 本体 lib 全件: `.\scripts\test-lib-full.ps1` (PC 共通ロック `miv-locks\test-lib-full` 取得下、`e85c702ef`)、
  **17,185 件すべて成功、75 skip、854 秒、exit 0**。実行中にマニュアルだけの commit (`d05408552`) を入れたため、
  スクリプトの再利用記録は残っていない (「Inputs changed during execution」)。製品コードは同じ。
- 既存の別問題: `cargo check -p mimageviewer --lib --tests` の統合テスト `tests/catalog_integration.rs` が master の §1.718
  (`CoverIdentity` 引数追加) に追従しておらずビルドできない。今回の変更とは無関係で、本ブランチでは直していない (`test-full.ps1` は止まる見込み)。
- 製品は起動していない。実機の挙動は未確認。

### 8.7 §9 ツールバーのお気に入り・ピン留め本の追補実装 (4a87140f3 でコミット)

変更ファイル: 製品コードは `src/app.rs` / `src/app/update_phases.rs`、テストは
`src/app/location_jump.rs` / `src/ui_main.rs`。文書は本書、`docs/everything-search-plan.md`、
`docs/search-architecture.md`、`docs/keymap-spec.md`、`docs/key-customization-impl-plan.md`、
`docs/next-release-backlog.md`、`htdocs/mimageviewer/manual/search.html` / `shortcuts.html` の計 12 ファイル。

- 設計の前提を source で照合した。ツールバーのお気に入りのボタン / ドロップダウンとピン留め本の open は
  同じ `toolbar_fav_nav` を返し、メニューバーは別の `fav_nav`。dispatch 前の先閉じだけが 3 検索 owner の
  saved_folder を捨てて close していた。前提の相違・新しい pending / bool / detached 遷移の変更はない。
- `src/app.rs` の toolbar 描画後から、global_search / favsearch / tag_view の saved_folder 消去と close を撤去した。
  `show_search_bar` (Ctrl+F) の先閉じ・local filter の取消 / 再投影は従来のまま残した。検索入力部品は変更していない。
- `src/app/update_phases.rs` の `fav_nav.or(toolbar_fav_nav)` を、優先順位を保った明示の分岐に分けた。
  `fav_nav` があれば既存 Direct、次に `toolbar_fav_nav` があれば dispatch 時の 3 owner の active を判定する。
  検索中だけ既存 `dispatch_location_jump(Physical(path))` を呼び、★固定・メニュー除外共通 modal・
  Everything 取得中・document-open admission と、成功時だけの `ExitTemporaryView` 採用を共有する。
  検索なしは既存 `Some(path)` の Direct tail のまま。拒否時も低優先候補へ流さず、そのフレームの勝者を維持する。
- ピン留め本の producer は変更しない。同じ `toolbar_fav_nav` なので追加の入口や状態を作らず、この規則が適用される。
  メニューバー・キー・リング・ゲームパッドのお気に入り、アドレス欄・フォルダペインは従来のまま。
- manual `search.html` / `shortcuts.html` と関連する現行仕様を、ツールバーのお気に入り・ピン留め本も成功時終了に含める記述へ更新した。
- テスト追加は 6 件。`src/app/location_jump.rs` に検索 4 種の成功 / 不存在 / scan 失敗、Plain Direct 比較、
  menubar > toolbar > keyboard > ready-folder の優先順位と menubar の旧検索履歴、一般 modal / Everything 取得中、
  検索後に★固定へ切り替えた候補の拒否を追加した。成功時は実際の HistoryBack の受付・非同期採用まで通す。
  `src/ui_main.rs` は headless な実 toolbar の描画・クリックを 4 検索 × ボタン / ドロップダウン / 実ピン留め本で行い、
  返る tuple の第 1 要素から同じ toolbar dispatch / scan 採用 / HistoryBack へ接続する。
  UI test から App 内の非公開 frame phase を呼ぶ bridge だけを `cfg(test)` 内に設け、製品側の入口や field は増やさない。

検証対象はこの追補差分、Windows / MSVC / 通常 feature set。製品起動・コミット・full suite は行わず、
`build-dev.ps1` は利用者の指示で設計担当へ任せる。Cargo 実行は `MSBUILDDISABLENODEREUSE=1`、
`CARGO_BUILD_JOBS=4` を使用する。

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(toolbar_favorite)' --no-fail-fast` | exit 0、7 実行 / 7 成功 (新規 6 件 + 既存 folder-pane/toolbar 優先順位 1 件)、4.435s、compile 2m44s、17,259 skipped。`target/search-nav-toolbar-new-tests.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(list_order) \| test(everything) \| test(location) \| test(gamepad) \| test(jump) \| test(section1328) \| test(toolbar_favorite)' --no-fail-fast` | exit 0、729 実行 / 729 成功、72.656s、compile 1.01s、16,537 skipped。`target/search-nav-toolbar-focused.log` |
| `python scripts/check_ui_glyphs.py` | 成功、危険 glyph 0 件 |
| `cargo fmt` (workspace 全体、引数なし) / `cargo fmt --check` | 最終実行も成功、exit 0 |
| `git -c core.safecrlf=false diff --check` | 最終整形後も成功、exit 0。`ui_main.rs` の差分は既存の test module 内 1 hunk だけで、検索入力 / 履歴▼ の差分はない |

別 context の read-only 独立実装レビュー (Sol / xhigh) は **ACCEPT、追加指摘なし**。
検索外 Direct と menubar、Ctrl+F の先閉じ、入力優先順位の維持と、6 件の追加テスト・公開文書の §9 との一致を検査した。
レビュー担当は Cargo / 製品起動 / 編集を行っていない。
製品ページのカードは変更せず、カードの上限検査は再実行していない。確認用 build は設計担当へ引き継ぐ。

### 8.8 §10 アドレス欄・利用者のフォルダペイン操作の追補実装 (ed6429060 でコミット)

設計の 4 種のアドレス Enter と利用者 pane 操作の前提を source で照合し、実装前の別 context の
Sol / xhigh 独立レビューでも routing・受付・ready restore 継承を確認した (合意、阻害する指摘なし)。
App の pending / bool、detached 述語 / viewport / registry 遷移、内部 scan の契約は増やしていない。

- アドレス Enter だけ `AddressBarNav::AddressInput(LocationJumpTarget)` として運ぶ。空欄は DriveList、
  本棚 root / 本棚内の名前 / 解決パスは Physical で、UI の TextEdit / IME 処理は変更しない。
  input/address・context・grid・immediate の全 4 dispatch から `dispatch_address_input` へ渡す。
  dispatch 時に検索中なら既存 `dispatch_location_jump`、検索なしなら既存の Plain downstream match を共有する。
  Plain の Direct / DriveList の引数・受付・history に共通 modal 等の新しい gate を足さない。
- `start_folder_pane_open` は変更せず、新しい利用者専用入口 `start_user_folder_pane_open` から既存の
  `replace_ready_folder_pane_open` へ合流する。利用者の受付時に検索中なら既存の JumpToPhysicalFolder purpose、
  それ以外は PaneNavigation を選ぶ。検索中は場所ジャンプと同じ既存 admission を共有する。
  ready の diagnostic と EPUB restore 継承を維持し、受付拒否時は継承 restore を既存 Abandoned 処理で終了する。
  要求は既存の MainListNavigation に ExitTemporaryView を所有し、成功採用でだけ検索を閉じる。
- 利用者入口の全一覧:
  1. `render_folder_pane_tree` の行クリック → `render_main_bars` の `folder_pane_nav` → main frame arbiter → replace-ready。
  2. `handle_folder_pane_keyboard` の Enter → start-user。
  3. `handle_gamepad_grid_accept` の pane Open (ゲームパッド A) → start-user。close は従来の Hide だけで二重 open しない。
  4. pane のカーソルを移して閉じる HideAndNavigate → `take_folder_tree_pane_close_navigation` →同じ frame arbiter。
  優先順位・ready 回収・同じパスの重複回避を維持する。
- 内部・別機能の `start_folder_pane_open` 呼出しは変更しない。クリップボード取り込み reveal の親一覧 open、
  取り込み先の場所ボタン、画像フォルダ ZIP 化の親一覧再openを保持する。起動復元・保存済みグループ等の
  内部 scan / 既存テストも利用者専用入口へ置き換えない。
- 検索が成功採用で終了するため、アドレス欄・pane から移った後は Esc で検索結果へ戻る従来の使い方がなくなる。
  履歴 Back は検索前の場所へ戻る。manual と現行仕様へ明記した。範囲外のお気に入り / メニューバー本棚等の挙動は維持する。

無反応調査の初回テストは、grid_physical_navigation だけを直接呼び、画像フォルダの自動 page open 意図を設定する
実際の accept_grid_container_open を飛ばしたため、キーへ届く前に本の open 待ちで失敗した。製品の原因特定の証拠ではない。
再現テストは実共通受付・grid dispatch・通常完了 poll へ訂正し、Everything Root / PhysicalChild × ZIP / 画像フォルダ本 × ↑/↓を検査する。
この 8 ケースは修正前の製品でも成功し、position=None では案内が出る。lock を報告原因とする根拠は得られていない。
追加調査は、保持した親 Folder 行の配下で現在の本自身を次の結果行として選ぶ場合、一覧端と案内表示設定、
同じ fixture の通常フォルダでの移動を、実 handler / worker / 採用で分けて検査する。
追加の実 handler 再現で、親行 A とその配下の本 B が両方結果にあると、A の基準を保持したままお気に入り経由で
B を読んでいる状態の次入力は、B 自身を再採用して基準 / index を A→B へ進める。画面の本は B のままで、
従来はこの成立した採用にも案内がなかった。採用後の Info を要求するテストは修正前に None で失敗した。
これは `list_order_position=None` の拒否や lock による取りこぼしではない。

修正は `apply_folder_nav_result` の scan 失敗処理後。ListOrder の宛先 logical path と現在の effective_folder の
physical key が **両方 Some で一致**するとき、既存の Info toast で「一覧の移動先は現在表示中の本です」と伝える。
行採用・帰還基準の更新・既存の前後順は変更しない。選ばれた宛先が同じ本であるという現在の事実を伝えるもので、
採用成功の先取りではない。新しい pending や通知輸送は作らず、通常 close / reopen 後も残る既存 toast を使う。
この限定 Info の設計は別 context の Sol / xhigh レビューでも合意済み。

別のケースとして、保持した列が親行 A だけなら worker は target=None を返して ListOrder Edge を出す。
既存の「先頭 / 末尾の案内を表示」が OFF の場合は、その Edge が意図どおり非表示になる。ON/OFF の実 handler テストを残し、
この設定は変更しない。サブ PC 担当のデータ配置と表示設定は記録がなく、どのケースが報告と一致するかは未確認。
コードで再現した「同じ物理本へ別行として進む」説明不足と、報告環境の原因の断定は分ける。

既存テストの変更は次の 6 件で、実アドレス Enter が返す marker を Direct / DriveList から AddressInput に追従する。
宛先・入力文字列・IME 合成 / 確定・フォーカス・履歴等の既存 assert は削除・緩和していない。

1. `section1684_address_enter_preserves_empty_and_bookshelf_routing`: 空欄と本棚 root / 名前 / 解決パスの候補型を変更するため。
2. `section207_address_enter_focus_reorder_escape_and_hidden_lifecycle`: Enter の physical / drive 候補型を変更するため。Escape / 非表示 / focus の検査は維持。
3. `address_ime_commit_and_immediate_enter_same_pass_navigates_and_releases_focus`: 同一 pass の IME 確定後 Enter の address marker を変更するため。
4. `address_ime_commit_and_immediate_enter_next_pass_navigates_and_releases_focus`: 次 pass の IME 確定後 Enter の address marker を変更するため。
5. `address_bar_focus_recovery_then_enter_navigates_and_releases_focus`: focus 回復後 Enter の address marker を変更するため。
3〜5 は共有 helper の address branch だけを変更し、同じ helper の検索入力 branch は従来どおり。

6. `section1339_address_bar_zip_enter_keeps_copied_path_across_row_refresh`: 実 Enter の旧 Direct marker と手動 Direct consumer を AddressInput と実 `dispatch_address_input` に追従するため。ZIP preflight / 行 refresh / 履歴の assert は維持する。初回の広い focused 実行で期待型の不一致を検出した。

変更ファイルは 22 件。製品は `src/app.rs`、`src/app/location_jump.rs`、`src/app/update_phases.rs`、
`src/app/gamepad_input.rs`、`src/ui_folder_pane.rs`、`src/ui_main.rs`。テストはそれらの test module に加え
`src/app/folder_pane_refresh_tests.rs`、`src/app/list_order_everything_tests.rs`、`src/app/tests.rs`。
文書は本書、architecture-overview / everything-search-plan / search-architecture / keymap-spec /
key-customization-impl-plan / list-order-book-navigation-plan / next-release-backlog と manual の
search / shortcuts / grid / gamepad / fullscreen。

新規テストは 11 件。address / pane の失敗・取消・stale、Plain / 内部 scan 回帰、pane gamepad と ready replacement の app 5 件、
4 検索 × 4 address 入力と vanished target の実描画 Enter 1 件、4 検索 × pane 行クリック / Enter と Back の実描画 1 件、
無関係な検索行からの案内・親行だけの Edge 設定・同じ本の別行への採用・検索終了後の通常 Folder 移動の実 handler 4 件。
最後のテストは Everything Root / PhysicalChild × ZIP / 画像フォルダ本について、実 AddressInput dispatch・scan 採用で検索を終了してから
通常 Folder の実キー一覧順移動・worker / 採用を確認する。独立 Sol / xhigh は製品差分とこの最終拡張を ACCEPT (追加指摘なし)。

製品起動・コミット・full suite は行わず、`build-dev.ps1` は利用者の指示で設計担当へ任せる。
Cargo 実行は `MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=4`。

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(list_order_everything_menubar_favorite_physical_book_reports_unavailable_from_real_key_handler)' --no-fail-fast` (初回) | exit 100、1 実行 / 0 成功 / 1 失敗、30.633s、compile 4m30s。上記 fixture の準備不足。`target/search-nav-address-pane-silent-red.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(list_order_everything_menubar_favorite_physical_book_reports_unavailable_from_real_key_handler) \| test(address_input) \| test(user_pane) \| test(internal_pane_scans_keep_transparent_search_navigation) \| test(folder_pane_user_search)' --no-fail-fast` | exit 100、8 実行 / 6 成功 / 2 失敗、38.103s、compile 8m31s。修正した無関係行の再現 8 ケースと app 5 テストは成功。headless 2 件は、既存 resolver が不存在 path を既存祖先へ解決する契約と、ペイン実入力 fixture の完了待ちを訂正した。`target/search-nav-address-pane-repro-tests.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(list_order_everything_menubar_favorite_physical_book_reports_unavailable_from_real_key_handler) \| test(list_order_everything_retained_parent_favorite) \| test(list_order_regular_folder_repro_fixture) \| test(address_input) \| test(user_pane) \| test(internal_pane_scans_keep_transparent_search_navigation) \| test(folder_pane_user_search)' --no-fail-fast` | exit 100、11 実行 / 9 成功 / 2 失敗、10.202s、compile 2m43s。同じ本への行採用で Info がない RED を取得。残る pane fixture は、単独描画では実検索欄の focus-loss pass がないため Everything の初回 focus 予約が残り Enter が再検索へ送られたもの。既存の focus retirement API でその既知予約だけを終了するよう訂正し、producer / 結果 / modal をテストから操作しない。`target/search-nav-address-pane-confirmed-red.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(list_order_everything_retained_parent_favorite_current_book_reports_its_nested_result_row) \| test(folder_pane_user_search)' --no-fail-fast` | exit 0、2 実行 / 2 成功、17,275 skip、5.910s、compile 5m41s。`target/search-nav-address-pane-corrections.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(list_order) \| test(everything) \| test(location) \| test(gamepad) \| test(jump) \| test(section1328) \| test(toolbar_favorite) \| test(address) \| test(folder_pane) \| test(user_pane) \| test(internal_pane) \| test(startup_restore) \| test(saved_group)' --no-fail-fast` (初回) | exit 100、957 実行 / 956 成功 (1 leaky) / 1 失敗、16,320 skip、101.886s、compile 4m34s。上記 section1339 の旧 marker / consumer を訂正。追加 11 件は成功。`target/search-nav-address-pane-focused.log` |
| 上記 957 件と同一の nextest コマンド (最終、section1339 訂正・workspace fmt 後) | **exit 0、957 実行 / 957 成功 / 0 失敗、16,320 skip、102.720s、compile 3m02s**。`target/search-nav-address-pane-final.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険な文字 0 件。 |
| `python scripts/check_site_cards.py` | exit 0、53 cards checked、0 over the limit。 |
| `.\scripts\test-focused.ps1 -PlanOnly` | shared / unmapped paths を検出しテスト実行なし、runner exit 1。禁止された full suite へ進まず上記明示 filter を使う。 |
| `cargo fmt` (workspace、引数なし、最後に再実行) / `cargo fmt --check` | ともに exit 0。検索入力部品と履歴▼の周辺に整形差分なし。 |
| `git -c core.safecrlf=false diff --check` | exit 0。 |

### 8.9 ed6429060 の全件テスト失敗 2 件の調査と修正 (9373116d1 でコミット)

入力証拠は利用者提供の、ロック下で実行された全件 nextest の
`target/test-lib-full/runs/20261111T101847481Z-ec099034/stderr.log`。
その実行は 17,202 件中 17,200 成功・1 失敗・1 timeout、974.069s、75 skip であり、全件成功とは扱わない。
本セッションでは全件を再実行せず、指定された単独・focused のみを実行する。
変更ファイルは `src/app/tests.rs` と本書のみ。製品動作と性能テストは変更しない。

1. `section1339_pane_enter_scan_crosses_real_search_refresh`:
   - `handle_folder_pane_keyboard` → `start_user_folder_pane_open` → replace-ready が、検索中の要求を
     `JumpToPhysicalFolder` として開始する。`resolve_main_folder_open_ready` の同 purpose 分岐は
     `apply_jump_to_physical_folder_ready_with_navigation` で採用を処理してから **None を返す**。
     普通の `PaneNavigation` が返す `ResolvedMainFolderOpen` を unwrap して再度手動 load する旧テストの consumer が不一致だった。
     同じクエリの結果再公開による取消ではない。
   - 同じクエリ・空の nav_stack の `execute_favsearch` は surface-switch 退役を行わない。
     修正テストは実 refresh helper の再公開を挟み、元の `MainListNavigation` が current のまま、cancel が false、
     pending が同一の navigation を保持することを明示的に検査する。
   - ready の手動 poll / unwrap / load をやめ、実 `post_render_main_navigation` の通常 frame consumer で完了まで処理する。
     成功後の `current_folder = directory`、本の行の存在を維持し、旧 `favsearch.nav_stack.last() = directory` は
     利用者承認の §10 の仕様に従い `favsearch.active = false` へ変更する。
     実履歴 Back で検索前の origin へ戻り、検索は閉じたまま、Forward が移動先を指すことも追加した。
     Back 用の origin はテスト専用の実ディレクトリを作る。
   - `rg` で section1339 / pane / search-refresh と resolver の同形 consumer を棚卸しした。
     `section1339_search_refresh_crossing_pending_folder_scan_keeps_copied_destination` は既に消費型 jump として処理している。
     required-fullscreen の再公開テストは消費型 None、ZIP preflight / classification の再公開テストは別の継続経路である。
     `independent_folder_pane_intent_retires_offscreen_smart_request_before_poll` の unwrap は検索からの利用者操作ではなく
     明示 `PaneNavigation` の内部 scan を対象にしており変更しない。これらを含む指定の合併 filter で検証する。
2. `section1375_large_physical_scan_10k_100k`:
   - 変更前の HEAD `ed6429060` で単独 filter を実行し、**1/1 成功、84.240s、exit 0**。
     10k は scan 68.421ms / projection 52.030ms、100k は 650.156ms / 528.737ms。
   - `git diff e85c702ef..HEAD -- src/app/folder_scan.rs src/app/book_materialization.rs Cargo.toml Cargo.lock .config/nextest.toml`
     は差分なし。テストは App を生成せず、ローカルファイル作成 → scan → pure projection → drop だけを行い、
     §9 / §10 の toolbar / address / pane dispatch を呼ばない。今回の製品差分が timeout の原因である根拠はない。
   - 100k の eprintln 後には listing の破棄、worker が所有する `TempDir` の drop、worker join が残る。
     Cargo.lock の tempfile 3.27.0 の `TempDir::drop` は `std::fs::remove_dir_all` で全ファイルを削除するため、
     出力は一時ファイル削除の完了を意味しない。ファイル作成・削除は表示した scan / projection の計測外。
     nextest の既存設定にもこのテストの過去最大 430s と 600s timeout が記録されている。
   - 単独では timeout を再現しなかったため、元の 600s の OS 内待機箇所・負荷要因までは断定しない。
     今回のナビゲーション差分とは無関係な時間超過として性能側を変更しない。timeout 延長・retry・計測の緩和も行わない。

Cargo は `MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=4`。

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(section1375_large_physical_scan_10k_100k)' --success-output immediate --no-fail-fast` | exit 0、1/1 成功、17,276 skip、84.240s、compile 1.46s。`target/search-nav-full-followup-scan.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(section1339_pane_enter_scan_crosses_real_search_refresh)' --no-fail-fast` (初回) | exit 101、テスト未実行。追加 enum 参照の修飾不足による E0433 を検出し、既存と同じ `super::FolderOpenScanPurpose` に訂正した。`target/search-nav-full-followup-pane.log` |
| 上記ペイン単独コマンド (訂正後) | exit 0、1/1 成功、17,276 skip、3.523s、compile 3m52s。元要求の保持・実採用・favsearch 終了・Back / Forward を確認。`target/search-nav-full-followup-pane-final.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(section1339) \| test(pane) \| test(folder_pane)' --test-threads 6 --no-fail-fast` | **exit 0、593/593 成功、16,684 skip、102.267s、compile 2.40s**。`target/search-nav-full-followup-focused.log` |
| `cargo fmt` (workspace、引数なし、テスト終了後) / `cargo fmt --check` | ともに exit 0。 |
| `git -c core.safecrlf=false diff --check` | exit 0。差分は上記 2 ファイルのみ。 |

製品起動・コミット・`test-lib-full` / `test-full` は行わない。全件 gate の再確認は設計担当へ引き継ぐ。

### 8.10 受入 (設計担当 ClaudeCode、2026-11-11、§9・§10)

- 独立レビュー (GPT-6.1 Sol / xhigh、範囲 `4a87140f3^..ed6429060`): **ACCEPT**、P1〜P3 なし。一覧順キーの押下から移動までの
  早期終了を全列挙し、`position` の `None` は共通 handler の案内に届くこと、scope / capture の除外による別の無通知経路は無いことを確認。
  同じ本への移動の案内は症状パッチではないと判断。サブ PC の観測の原因と一致した証拠は無い (入力所有・準備中 lock の可能性は実機資料なしでは確定できない)。
  メニューバーの閲覧履歴・評価一覧は既存の採用処理で検索を閉じるため、範囲外で擬似子のまま残るのはお気に入り (ツールバー以外)・メニューバーの本棚。
- `test-lib-full` (PC 全体ロック下):
  - 1 回目 (`ed6429060`): 17,200 成功・1 失敗・1 タイムアウト。失敗は §8.9 の 1 (テストの手動 consumer、`9373116d1` で修正)。
    タイムアウトは section1375 の 100k scan 性能テスト (単独 84 秒で成功、今回の差分を通らない)。
  - 2 回目 (`9373116d1`): 17,201 成功・1 失敗。失敗は `caption_rotation_rename_move_display_then_reset_preserves_reused_path_and_sibling` の
    ディレクトリ rename の `PermissionDenied` (OS error 5)。今回の差分を通らず、単独では成功。
  - 3 回目 (`9373116d1`): **17,202 件すべて成功**、75 skipped、849.7 秒、exit 0。
- `.\scripts\build-dev.ps1`: 成功 (`target\dev-runtime\mimageviewer-core.exe`、2026-11-11 20:32)。
- 実機確認の記録はまだ無い。

### 8.11 §11 の実装・検証記録 (未コミット)

着手前 source 調査で、typed お気に入りには通常の 4 consumer に加えて native backdrop 内の consumer も必要と判明し、
一度設計担当へ報告した。**設計担当・利用者の判断で、この経路は detached ではなく main frame の navigation consumer であり、
凍結対象外、§11 に含めて実装する**と確定して再開した。LocationJump 等の既存の取りこぼしも含めて全 variant を棚卸しした。
以下は source / headless test の記録であり、サブ PC 担当以外の実機観測を主張しない。

#### 着手前調査と main consumer の修正

- `app.rs:100365` の `poll_video_presentation_transition` は gamepad dispatch (`100747`) より前。
  お気に入り確定は `gamepad_input.rs:5509` で `close_fullscreen()` を呼ぶが、表示切替中は
  `app.rs:82313` が `TerminalClose` を積んで戻り、その場で `fullscreen_idx` を消さない。
- 実 teardown は `native_video.rs:3111` の `TerminalSessionClose` effect。
  その後の fullscreen native render (`ui_fullscreen.rs:27516`) はこの effect を処理しない。
  Fullscreen → MainWindow 切替中なら、Video / fullscreen index が残り
  `native_video.rs:4826` の backdrop 判定は true になり得る。
- この先の `app.rs:101017` は Direct の partial consumer。新しい `AddressBarNav::Favorite` を通常 4 箇所だけに足すと、
  ここで要求が消える。したがって「producer が close するので native consumer には到達しない」という前提は成立しない。
  これは source による反証であり、サブ PC の新しい観測や本セッションの native 実行結果ではない。
- 最小案は、この既存 consumer に Favorite の arm を足し、同じ favorite dispatcher を呼ぶだけ。
  dispatcher は検索中なら既存 LocationJump、検索外なら path を旧 Direct consumer に返す。
  immediate / native の Plain は従来の `_owned` loader と `clear_pending_folder_nav_steps()` を保つ。
  viewport 述語・lifecycle・registry・表示切替 reducer / effects の変更や、新しい App 状態は不要。
  この native 経路の consumer 完備は上記判断で承認済み。実装は variant ごとの if-let を廃し、
  `dispatch_native_backdrop_navigation` → exhaustive な `dispatch_main_input_nav_immediate` に集約した。
  通常の fullscreen-close の即時 dispatcher も同じ本体へ委譲する。native の旧 gamepad 優先順位は保ち、
  gamepad が無い場合の keyboard、どちらも無い場合の folder worker 結果も処理する。
  native player を作らない Video metadata fixture で、切替中の real picker 確定・backdrop 判定・実 consumer を検査する。

通常のメニュー・toolbar・key/ring/gamepad お気に入りは共通 `dispatch_favorite_navigation` を通る。
検索中だけ既存 `dispatch_location_jump(Physical)` の受付・成功時終了を使い、Plain は path を各旧 Direct consumer へ返す。
メニューの返り値は `Option<LocationJumpTarget>`、キー等は `AddressBarNav::Favorite(PathBuf)`。
本棚 root は描画時 Plain なら旧即時 open、検索中なら BooksRoot の候補を返し、dispatch 時に再判定する。
これにより検索なしの同一 frame のメニュー順序も変更しない。新しい App pending / bool / 次回採用フラグはない。

#### native main consumer の全 variant 棚卸し

新しい `Favorite` を含めて `AddressBarNav` は 20 variant。native の consumer は wildcard / 未知型の破棄ではなく、
全 variant を処理する `dispatch_main_input_nav_immediate` を呼ぶ。既存の gamepad 優先順位を保ち、keyboard 勝利時は
既存 `keyboard_nav_from_fullscreen_close` も保持する。下記 source 行は c76356bac 基準、Favorite は今回の新型。
GridOnly の入口も同じ frame / batch の viewer 変更があり得るため、ここでは型を理由に破棄しない。

| Variant | gamepad_nav への producer / 文脈 | 共通 dispatcher での処理 |
| --- | --- | --- |
| Favorite | favorite picker South (`gamepad_input.rs:5370,5483,5518`)、West ring → slot (`4927,7464,8069,7652`)。番号 / 前後キーも同 helper | `dispatch_favorite_navigation`。検索中は LocationJump、Plain は旧 Direct `_owned` + steps 消去 |
| LocationJump | location picker South (`5535,5599,5634`)、drive / place ring (`7698,7720,7756,7791,8085`)。全 ring 文脈 | `dispatch_location_jump`。§2.1 で型付き化した既存場所入力の native 取りこぼしも解消 |
| Direct | grid South → `collection_grid.rs:1435,1480`、grid East / parent → `grid_parent.rs:585`、smart/subfolder parent | 旧 Navigation `_owned` loader + steps 消去 |
| GridVirtual | grid South、GridOpenSelectedAsPage/List ring → `app.rs:61733` / `collection_grid.rs:1456` | `start_grid_virtual_open` + steps 消去 |
| CollectionSource | grid South (`collection_grid.rs:1469`)、grid East parent (`1244`) | 旧 CollectionGridPhysical `_owned` loader + steps 消去 |
| RatingSource | grid South (`collection_grid.rs:1475`) | `start_rating_physical_open`。native の旧 steps 消去も維持 |
| DriveList | grid East / GridParent ring (`grid_parent.rs:593`) | `enter_drive_list_from_navigation` |
| ReadingHistory | grid East (`grid_parent.rs:540` → `app.rs:24636,24646`) | `enter_reading_history_with_entrance` |
| Bookmarks | grid East (`grid_parent.rs:534` → `app.rs:24655,24660`) | `enter_bookmark_view` |
| ReadingHistoryRootExit | grid East (`grid_parent.rs:568` → `reading_history_origin.rs:237`) | `exit_reading_history_root` |
| Collection | grid East (`grid_parent.rs:518` → `collection_grid.rs:1256`) | `apply_collection_input_nav`。gamepad は false、keyboard は既存の close-origin を渡す |
| HistoryBack | 全 ring 文脈 (`ring_shortcut.rs:1481,1507,1549`、`gamepad_input.rs:8335,7624,7643`)、grid East bookmark root (`grid_parent.rs:548`) | `dispatch_main_folder_history_input(Back)`。返り値に物理 path があれば旧 Direct へ渡す |
| HistoryForward | 全 ring 文脈 (`gamepad_input.rs:8336,7624,7641`) | 同じ共通履歴受付 (Forward) |
| ZipListReturn | gamepad_nav の直接 producer はない。viewer East / CloseFullscreen は pending close → fullscreen_close_nav (`close_return.rs:361,416`) | 受けた場合も `apply_zip_list_return` で処理 |
| EverythingResults | 直接 producer なし、viewer close の帰路 (`close_return.rs:370`) | `return_from_everything_child(Results)` |
| RatingViewBack | 直接 producer なし、grid East はその場で parent 処理し None、viewer close の帰路 (`close_return.rs:382`) | `rating_view_parent_nav` |
| BooksRoot | raw variant の gamepad producer なし。location picker / ring は LocationJump(BooksRoot) | `open_books_root` |
| AddressInput | gamepad producer なし、address Enter (`ui_main.rs:16505`) | `dispatch_address_input` |
| CollectionOpen | gamepad / ring producer なし、keyboard saved-group (`app.rs:59245` → `saved_group_actions.rs:456`) | `open_collection_grid_from_navigation` |
| SavedGroupReady | gamepad_nav の直接 producer なし、非同期 saved-group ready の別 lane (`saved_group_actions.rs:605,620` → `update_phases.rs:1185`) | `adopt_saved_group_ready` |

Select / Start / analog / marker / ring-settings picker 自身は nav を返さず overlay 等を操作する。
location picker の Bookmarks はその場で `open_bookmark_browser` を呼び None を返す。
既存 `apply_fullscreen_close_nav_immediate` の history / saved-group を false で通常 lane へ返す契約は保持するが、
native はその部分 dispatcher を使わず上表の total 本体を直接呼ぶため、未処理型が無言で落ちる経路はない。
viewport 述語・lifecycle・registry・reducer / effects は変更していない。

#### 入口の全列挙 (c76356bac の source。今回の変更と残存を区別)

| 利用者入口 | 経路・検索の扱い | §11 の扱い |
| --- | --- | --- |
| メニューバーのお気に入り | `ui_main.rs:8836` → fav_nav → Direct、検索を保持していた | 共通 favorite helper、成功時終了 |
| ツールバーのお気に入り（ボタン・ドロップダウン）・ピン留め本 | `ui_main.rs:12078,12098,12378` → toolbar_fav_nav、検索中は成功時終了 | 同じ favorite helper へ統一 |
| キーのお気に入り番号・前後 | `gamepad_input.rs:7796,7811,7812,7886` → slot helper (`7652`) → Direct だった | typed Favorite、成功時終了 |
| リング・マウスボタン・ジェスチャのお気に入り | `gamepad_input.rs:3521,3549,7620,8069` → 同じ slot helper | 同上 |
| ゲームパッドのお気に入り一覧 | `gamepad_input.rs:5483,5518` → Direct だった | 同上。native consumer も全 variant 対応 |
| メニューバーの本棚 root | `ui_main.rs:8972` → `app.rs:54910` → load、検索を保持していた | 検索中は typed BooksRoot。Plain は描画中の即 open を維持 |
| アドレス Enter | `ui_main.rs:16505` → AddressInput | §10 対応済み。未解決時の通知を両モードへ追加 |
| ペインのクリック・Enter・gamepad・カーソルを動かして閉じる | `app.rs:60845,61089`、`ui_folder_pane.rs:381`、`gamepad_input.rs:9532` | §10 対応済み。固定キーの native / egui 所有を共通化 |
| 場所▼の通常項目・整理先、場所キー・リング・マウス・gamepad場所一覧 | `ui_main.rs:16123–16228`、`gamepad_input.rs:5618,7698,7720,7756,7791` | 既存 LocationJump、成功時終了 |
| 実際の親へ・コンテキストの場所へ移動 | `grid_parent.rs:430`、`app.rs:60623` | 既存 ExitTemporaryView、成功時終了 |
| ファイル → フォルダを開く | `ui_main.rs:8640` → `ui_dialogs/open_folder.rs:102` → `update_phases.rs:1338` の Direct | **検索を保持する残存入口。今回の承認範囲外として変更しない** |
| 追加先の本を開く・本の番号・本の前後 | `saved_group_actions.rs:132,309,642` → owned scan/load。検索 source lease も受理 | **残存**。本棚 root とは別の操作 |
| 製本管理の「開く」 | `ui_main.rs:10027–10031` → open_books_root / load_folder | **残存** |
| 場所▼の「クリップボード取り込み」 | `ui_main.rs:13007,13017` → internal start_folder_pane_open | **残存**。通常場所項目とは別の経路 |
| 取り込み通知の「表示」 | `app.rs:20398,20417` → internal PaneNavigation | **残存** |
| アドレス欄の履歴▼ | `ui_main.rs:16368–16403`、検索中 disabled、Direct 候補 | 通常操作では対象外。同じ frame の後発検索との関係は未変更。並行作業領域のため触れない |
| 作業場所 A/B | `ui_main.rs:15848–15911`、検索中 disabled、workspace switch | 通常操作では対象外。同じ frame の後発検索との関係は未変更 |
| 履歴 Back / Forward | `app.rs:25301` で active search を拒否 | 検索を残す物理移動には該当しない |
| ブックマーク一覧とその行 | `app.rs:53762,53779` の一覧採用で検索を終了、その後 `54143` で行を開く | 残存入口には該当しない |
| 現在の本のブックマーク | `app.rs:53456` | 同じ本のページ移動、場所ジャンプ対象外 |
| 貼り付け・外部 D&D | `app.rs:63480,52161`、`update_phases.rs:405` | ファイル操作。検索結果の一覧では拒否 |
| Explorer で表示 | `context_menu.rs:1017` 等 | 外部アプリの操作、内部場所移動なし |
| Explorer / single-instance Activation からの open | `startup_ops.rs:290,376,1748` → physical loader | 外部 open の別仕様、今回変更しない |
| 検索結果の本・フォルダ、Everything PhysicalChild、検索内の戻る・親・前後 | `app.rs:35178,35197,38505` 等 | 設計で明示除外。検索の一部として保持 |

残存入口が検索を保持する source 根拠は、通常 physical load の成功尾部
`app.rs:38505` の `commit_staged_search_navigation_effects()` が active owner を検索の子へ進める点。
§11.1 の「他にも残っていないか列挙する」はこれらを一括変更する承認とは解釈せず、上表に明記する。

#### ペイン・アドレスの根因と修正

- ペイン: `ui_folder_pane.rs:359–383` は egui の raw consume 後に grid focus へ戻し、
  `app.rs:59441` の GridOpenSelected が残る native KeySlot twin を読む経路がある。
  固定キーを既存 `Keymap::consume_fixed_chord` に寄せ、両方の入力を同じ所有境界で消費するよう修正した。
  旧 egui NONE が許す Shift / Alt、Enter に折り畳まれる NumpadEnter、重複 press の 1 操作化を維持する。
  main frame の root keyboard → toolbar → grid shortcuts → 実検索バー / pane → grid → nav の順で流す 6 テストを追加した。
  最初の 5 失敗は root 行を描く fixture の準備段階であり、二重配送の再現の証拠ではない。
  cursor 初期化と他の一時ディレクトリを含まない pane 専用 root へ fixture を訂正した。
- アドレス: `ui_main.rs:16531` の resolver が None の場合に else がない。
  `git show master:src/ui_main.rs` の対応する Direct producer も同じ無通知分岐だった。
  新規 headless Enter テストの Plain ケースで、候補なし・一覧/選択/履歴を保持・toast=None を確認し、
  通知を要求する assert は失敗した。検索なしの既存無通知を再現した RED。
  未解決の非空入力には既存 warning で「場所が見つかりません: …」を出す。引用符付き空白の no-op と、
  存在しない子から既存の親へ移る resolver の動きは変えない。UI TextEdit / IME と履歴▼は変更しない。
- `menubar_search_jump_tests.rs` は実メニュークリックから 4 検索の成功 / 失敗、実 Back、Plain の即時 BooksRoot と Direct を検査する 6 件。
- `native_backdrop_navigation_tests.rs` は real picker 確定で TerminalClose が pending かつ backdrop=true の状態を作り、
  Favorite / LocationJump の採用・失敗・4 検索、Plain loader cleanup、History Back / Forward を検査する 6 件。
- `location_jump.rs` に、番号 / 前 / 次キー・ring・gamepad picker の成功 / 失敗を各検索で通す検査と、modal / snapshot / Plain immediate の 2 件を追加。
- 変更ファイルは製品コード `app.rs` / `ui_main.rs` / `ui_folder_pane.rs` / `app/{gamepad_input,location_jump,update_phases}.rs`、
  既存 `app/{tests,list_order_everything_tests}.rs`、新規 pane / menubar / native-backdrop の 3 test module、本書、関連仕様 8 件、manual 4 件 (計 24 ファイル)。

#### 変更ファイル 24 件

- `src/app.rs`
- `src/ui_main.rs`
- `src/ui_folder_pane.rs`
- `src/app/gamepad_input.rs`
- `src/app/location_jump.rs`
- `src/app/update_phases.rs`
- `src/app/tests.rs`
- `src/app/list_order_everything_tests.rs`
- `src/app/search_nav_pane_frame_tests.rs`
- `src/app/menubar_search_jump_tests.rs`
- `src/app/native_backdrop_navigation_tests.rs`
- `docs/architecture-overview.md`
- `docs/everything-search-plan.md`
- `docs/key-customization-impl-plan.md`
- `docs/keymap-spec.md`
- `docs/list-order-book-navigation-plan.md`
- `docs/next-release-backlog.md`
- `docs/search-architecture.md`
- `docs/search-result-navigation-682-688-plan.md`
- `docs/spec.md`
- `htdocs/mimageviewer/manual/search.html`
- `htdocs/mimageviewer/manual/shortcuts.html`
- `htdocs/mimageviewer/manual/grid.html`
- `htdocs/mimageviewer/manual/gamepad.html`


#### 新規テスト 21 件の一覧

| File | Test |
| --- | --- |
| `src/app/search_nav_pane_frame_tests.rs` | `search_nav_pane_frame_metadata_enter_opens_tree_target` |
| `src/app/search_nav_pane_frame_tests.rs` | `search_nav_pane_frame_everything_enter_opens_tree_target` |
| `src/app/search_nav_pane_frame_tests.rs` | `search_nav_pane_frame_favorite_search_enter_opens_tree_target` |
| `src/app/search_nav_pane_frame_tests.rs` | `search_nav_pane_frame_tag_enter_opens_tree_target` |
| `src/app/search_nav_pane_frame_tests.rs` | `search_nav_pane_frame_plain_enter_opens_tree_target` |
| `src/app/search_nav_pane_frame_tests.rs` | `search_nav_pane_frame_enter_preserves_modifiers_keypad_and_coalescing` |
| `src/app/menubar_search_jump_tests.rs` | `menubar_favorite_each_search_adopts_then_real_back_returns_before_search` |
| `src/app/menubar_search_jump_tests.rs` | `menubar_books_root_each_search_adopts_then_real_back_returns_before_search` |
| `src/app/menubar_search_jump_tests.rs` | `menubar_missing_favorite_each_search_keeps_results_selection_and_origin` |
| `src/app/menubar_search_jump_tests.rs` | `menubar_failed_books_root_each_search_keeps_results_selection_and_origin` |
| `src/app/menubar_search_jump_tests.rs` | `menubar_plain_books_root_opens_during_render_without_deferred_candidate` |
| `src/app/menubar_search_jump_tests.rs` | `menubar_plain_favorite_uses_existing_direct_navigation_and_history` |
| `src/app/native_backdrop_navigation_tests.rs` | `native_backdrop_gamepad_favorite_each_search_adopts_and_returns_before_search` |
| `src/app/native_backdrop_navigation_tests.rs` | `native_backdrop_gamepad_missing_favorite_each_search_preserves_source` |
| `src/app/native_backdrop_navigation_tests.rs` | `native_backdrop_gamepad_location_drive_list_and_physical_exit_each_search` |
| `src/app/native_backdrop_navigation_tests.rs` | `native_backdrop_gamepad_vanished_location_each_search_preserves_source` |
| `src/app/native_backdrop_navigation_tests.rs` | `native_backdrop_plain_favorite_and_direct_keep_immediate_loader_cleanup` |
| `src/app/native_backdrop_navigation_tests.rs` | `native_backdrop_history_back_and_forward_are_consumed` |
| `src/app/location_jump.rs` | `favorite_key_ring_and_gamepad_handlers_exit_each_search_only_after_success` |
| `src/app/location_jump.rs` | `favorite_typed_dispatch_rechecks_modal_snapshot_and_plain_immediate` |
| `src/ui_main.rs` | `address_missing_drive_enter_notifies_and_preserves_search_or_plain_state` |


#### 既存テストを変更した理由 (削除・弱化なし)

1. `toolbar_favorite_priority_keeps_menubar_then_toolbar_then_keyboard_ahead_of_folder_navigation`: menubar が勝つ優先順位は保持し、§11 で承認された検索終了を期待する。旧「検索を残す」assert だけを成功採用後の終了 / 検索前 history に変更した。
2. `location_jump_plain_inputs_use_same_transport_and_favorites_keep_direct_navigation` → `...favorites_defer_search_decision`: favorite producer の型を Favorite に変更したため。dispatch 前に検索を閉じない assert は維持。
3. `mixed_path_favorite_picker_direct_navigation_preserves_saved_record`: 新しい Favorite から path を取り出し、既存の ExplicitList intent と旧 Plain path 検査を維持。混在表記や拡張子付き実フォルダの assert は変更しない。
4. `list_order_everything_menubar_favorite_physical_book_reports_unavailable_from_real_key_handler` → `...folder_dialog_physical_book...`: menubar は検索を終えるため、検索を保持する同じ source 状態を作る範囲外の FileOpenFolder の実 frame consumer へ fixture を移した。Root / Child・ZIP / 画像フォルダ・上下の全 assert を維持。
5. `list_order_everything_retained_parent_favorite_book_reaches_boundary_notice_policy` → `...retained_parent_folder_dialog_book...`: 同じ共通 fixture の入口変更。Edge と表示 ON/OFF の検査は維持。
6. `list_order_everything_retained_parent_favorite_current_book_reports_its_nested_result_row` → `...retained_parent_folder_dialog_current_book...`: 同じ入口変更。基準 A→B・index・同じ本の Info・採用後の全 assert は維持。
7. `mixed_path_favorite_direct_navigation_standardizes_input_without_rewriting_saved_record`: 広い focused で旧 Direct marker の期待が残っていたことを検出。ring の候補を Favorite として受け、Plain は共通 helper の返す path を旧 Direct 検査へ渡す。menu / toolbar の保存 path も共通 helper を通す。保存表記・正規化・拡張子付き実フォルダ・分類・設定を書き換えない全検査を維持。
8. `section1339_pane_enter_scan_crosses_real_search_refresh`: 固定キーが共通 keymap owner を使うため、裸の pane handler 呼出しでは App pass の owner cache がなく fail-closed になる (keyboard_input.rs の既存契約)。本番の `input_root_keyboard` から Enter を流すよう fixture を訂正した。再公開前後の同一 navigation / current / cancel=false、実 frame 採用・favsearch 終了・本の行・Back / Forward の全検査は維持。新しい検索の取消や製品の採用不良ではない。

独立 Sol / xhigh の source review は ACCEPT。close-origin 引継ぎと native テストでの次 frame の transition poll を指摘され、
既存 flag の輸送と実順序の poll / terminal-effect 検査を補い、再確認で未解決 P1〜P3 はなかった。実行結果は下記で区別する。

Cargo は `MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=4`。

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(search_nav_pane_frame) \| test(address_missing_drive_enter)' --test-threads 2 --no-fail-fast` (初回) | exit 101、テスト未実行。fixture の不存在 zip_context と UI test の private snapshot field 参照を訂正。`target/search-nav-section11-red.log` |
| 同じコマンド (compile 訂正後) | exit 100、6 実行 / 0 成功 / 6 失敗、17,277 skip、1.730s、compile 3m27s。pane 5 件は fixture、address 1 件は Plain の無通知 RED。`target/search-nav-section11-red-corrected.log` |
| `cargo fmt` (workspace、引数なし、停止 checkpoint の最後) / `cargo fmt --check` | ともに exit 0。未登録の menubar test draft は compile / test 対象にしていない。 |
| `git -c core.safecrlf=false diff --check` | exit 0。tracked 15 件 + 新規 draft test 2 件、製品ロジックの変更なし。 |
| `cargo nextest run -p mimageviewer --lib -E 'test(search_nav_pane_frame) \| test(address_missing_drive_enter) \| test(menubar_) \| test(favorite_key_ring_and_gamepad_handlers) \| test(favorite_typed_dispatch) \| test(native_backdrop_) \| test(toolbar_favorite_priority)' --test-threads 4 --no-fail-fast` (再開後初回) | exit 101、テスト未実行。KeyName::Escape と payload のない FolderNavMode::ListOrder を訂正。`target/search-nav-section11-implemented.log` |
| 同じ再開後コマンド (型名訂正・close-origin / transition poll 補強後) | **exit 0、25/25 成功、17,273 skip、18.125s、compile 14m42s**。新規 21 件を含む。`target/search-nav-section11-narrow.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(list_order) \| test(everything) \| test(location) \| test(gamepad) \| test(jump) \| test(section1328) \| test(section1339) \| test(favorite) \| test(bookshelf) \| test(books_root) \| test(menubar) \| test(pane) \| test(address) \| test(backdrop) \| test(startup_restore) \| test(saved_group)' --test-threads 6 --no-fail-fast` (初回) | exit 100、1,992 実行 / 1,990 成功 / 2 失敗、15,306 skip、327.569s、compile 1.69s。上記 7・8 の旧 marker / pass fixture を訂正。`target/search-nav-section11-focused.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(mixed_path_favorite_direct_navigation_standardizes_input_without_rewriting_saved_record) \| test(section1339_pane_enter_scan_crosses_real_search_refresh)' --test-threads 2 --no-fail-fast` | exit 0、2/2 成功、17,296 skip、3.665s、compile 3m02s。`target/search-nav-section11-existing-fixtures.log` |
| 上記の 1,992 件と同じ合併 filter / threads の nextest コマンド (最終) | **exit 0、1,992/1,992 成功、15,306 skip、297.311s、compile 1.32s**。`target/search-nav-section11-final.log` |
| `python scripts/check_ui_glyphs.py` (再開後) | exit 0、危険文字 0 件。 |
| `python scripts/check_site_cards.py` (再開後) | exit 0、53 cards checked、0 over the limit。 |
| `cargo fmt` (workspace、引数なし、最終テスト終了後) / `cargo fmt --check` | ともに exit 0。検索入力欄・履歴▼への整形差分なし。 |
| `git -c core.safecrlf=false diff --check` (最終) | exit 0。tracked 21 件 + 新規 test module 3 件、計 24 ファイル。 |

再開後の実行結果は上表と後続追記で区別する。製品起動・full suite・コミットは行わない。
`build-dev.ps1` は利用者の既存指示どおり設計担当が実行する。

## 9. 追補: ツールバーのお気に入りもそろえる (2026-11-11 利用者決定)

利用者の判断 (親セッション経由、2026-11-11): §2.1 範囲外で相談事項にした「ツールバーのお気に入りは移動前に検索を閉じる」不揃いは、
**そろえる**。ほかの場所ジャンプと同じく「移動先を開けた時点で検索を終える。失敗・取消のときは検索結果を残す」にする。
同じ「移る前に検索を閉じる」不揃いを持つ入口があれば、同じ経路にそろえてよい (範囲を広げる場合は本書に書く)。

### 9.1 棚卸し (コード、2026-11-11)

| 入口 | 検索中の今の挙動 | 今回 |
| --- | --- | --- |
| ツールバーのお気に入り (ボタン列・ドロップダウン、`ui_main.rs` 約 12077・12097 → `toolbar_fav_nav`) | 描画直後 (`src/app.rs` 約 101181) に 3 owner を `saved_folder = None` + `close_*()` で **先に閉じて** から通常の Direct 移動。行き先が無効でも結果が消える | **そろえる** |
| ツールバーのピン留め本を開く (`ui_main.rs` 約 12377、同じ `toolbar_fav_nav`) | 同上 (同じ変数・同じ先閉じ) | **そろえる** (同じ経路なので一緒に直す) |
| メニューバーのお気に入り (`fav_nav`)・お気に入りスロット/前後 (キー・リング等)・ゲームパッドのお気に入りリスト | 先に閉じない。検索を残したまま擬似的な子として開く | 変えない (「移る前に閉じる」不揃いではない) |
| アドレス欄・フォルダペイン | 同上 | 変えない |

`close_*()` を移動の前に呼ぶ箇所は、ほかに検索 UI 自身の ×・Esc・切替・タグのトグル等だけで、場所移動の入口には無い。

### 9.2 決定

- `toolbar_fav_nav` の先閉じ (3 owner の `saved_folder = None` + `close_*()`) を外す。
- dispatch の時点 (`update_phases.rs` の `higher_priority_direct_nav` を処理する所) で、`toolbar_fav_nav` が勝ち、かつ検索 3 owner の
  どれかが active なら、§2.1 と同じ ExitTemporaryView の実フォルダジャンプ (`LocationJump(Physical(path))` と同じ helper、同じ
  dispatch 時の受付判定: ★固定・共通モーダル・Everything 取得中) へ渡す。検索が無ければ **今と同じ** Direct 移動のまま
  (受付・history・優先順位を変えない)。メニューバーの `fav_nav` は今の経路のまま (同じ変数を共有しているので区別して扱う)。
- Ctrl+F (現在地フィルタ、`show_search_bar`) の先閉じは 3 検索 owner ではなく今回の対象外なので、今の挙動のまま残す。
- 優先順位 (`fav_nav > toolbar_fav_nav > keyboard/gamepad_nav > folder_nav`) は変えない。
- テスト: 4 種の検索それぞれで、ツールバーのお気に入り (ボタン・ドロップダウン) とピン留め本を押すと、採用後に検索が閉じ移動先が出て、
  ← で検索前の場所へ戻る。存在しない行き先・scan 失敗では検索結果・選択が残る。検索なしでは今と同じ移動 (回帰)。
  メニューバーのお気に入りは今の挙動のまま (回帰)。
- マニュアル: search.html・shortcuts.html の「お気に入りには適用されません」等の記述を、ツールバーのお気に入りとピン留め本を含める形に直す。

## 10. 追補 2: アドレス欄とフォルダペインもそろえる (2026-11-11 利用者決定)

経緯: サブ PC 担当が `99f8cbdb8` を確認した際 (依頼 33 第 2 部、観測者はサブ PC 担当)、検索中にアドレス欄で別フォルダへ移ると
一覧は移動先になるのに「Everything: …」表示と検索バーが残り、そこで開いた本で Ctrl+Shift+↓ を押しても移動も案内も無かった、
Esc では検索結果へ戻った、と報告された。設計担当が「そろえると Esc で結果へ戻る使い方がなくなる (リリース済みの挙動の変更)」と
相談し、利用者が **そろえる** と判断した (親セッション経由、2026-11-11)。

### 10.1 決定

- **アドレス欄** (`ui_main.rs` 約 16505 の Enter: 空欄 → ドライブ一覧、「本棚」「本棚 > 名前」、解決したパス) は、dispatch の時点で
  検索 3 owner のどれかが active なら §2.1 と同じ ExitTemporaryView の場所ジャンプ (実フォルダ・ドライブ一覧) と同じ helper・
  同じ dispatch 時の受付判定へ渡す。検索が無ければ今と同じ Direct / DriveList のまま。
- **フォルダペイン** (利用者がペインの行をクリック / Enter で開く入口。ゲームパッドのペイン操作も同じ利用者操作なら含める) は、
  受け付けた時点で検索のどれかが active なら、明示ジャンプと同じ成功時終了 (`ExitTemporaryView`) の scan で開く。検索が無ければ今と
  同じ `PaneNavigation`。受付後に検索が始まった場合は、既存の `retire_main_list_requests_for_surface_switch` がペインの scan を取り消す
  (今と同じ)。起動時の復元・保存済みグループ・画像フォルダ ZIP 化など、ペインの scan を内部で使う呼び出しは変えない。
- 成功時: 検索を閉じて移動先を表示し、← で **検索前の場所** へ戻る (検索結果は履歴に残らない。§1.689 は保留のまま)。
  失敗・取消・stale: 検索結果を残す。
- これで「検索バーが残ったまま物理一覧を見ている」状態は、アドレス欄・ペインからは作られなくなる。
- **案内が出なかった原因も特定して直す**: 範囲外のまま残る入口 (メニューバー・キー等のお気に入り、ゲームパッドのお気に入りリスト、
  メニューバーの本棚) では、今も検索を残したまま物理一覧へ移れる。その状態から本を開いて Ctrl+Shift+↑/↓ を押したとき、
  `list_order_position` が `None` なら「一覧から開いた本で使えます」が出るはずなので、どの分岐で黙って終わったか
  (例: 結果の Folder 行の配下と判定されて移動が始まり、移動先が無いまま黙って終わる、受付の lock で落ちる等) を
  再現テストで特定する。黙って終わる分岐が見つかれば、通知を出すか正しく判定するよう直す (silent な早期 return を残さない)。

### 10.2 変わる挙動 (リリース済み) と更新履歴の文案

- 変わる挙動: 検索中にアドレス欄・フォルダペインで移ると、今までは検索を残したまま開き、Esc で検索結果へ戻れた。これからは
  移動先を開けた時点で検索を終える。検索結果へは戻れない (← は検索前の場所へ戻る)。
- 更新履歴の文案 (公開時に利用者が確認する): 「検索中に場所ジャンプ (デスクトップ・ドライブ・ドライブ一覧・閲覧履歴・本棚・
  レーティング一覧など)、アドレス欄、フォルダペイン、お気に入り (ツールバー・メニューバー・キー・リング・ゲームパッドなど)、
  メニューバーの本棚から移ると、移動先を開けた時点で検索を終えるようにしました。
  移動先で ← を押すと検索前の場所へ戻ります。移動に失敗したときは検索結果が残ります。」
  - 親セッションの案にあった「検索結果へは ← で戻れます」は実際の動きと異なる (← は検索前の場所へ戻り、検索結果は履歴に残らない) ため採らない。

### 10.3 テスト

- 4 種の検索で、アドレス欄 (パス・空欄・「本棚」・「本棚 > 名前」) とフォルダペイン (クリック・Enter) から移ると、採用後に検索が閉じ、
  ← で検索前の場所へ戻る。存在しない・読めない行き先では検索結果・選択が残る。検索なしでは今と同じ (回帰)。実描画→操作の headless を含める。
- 観測の再現: Everything で本を開いた後 / Root で、アドレス欄から別フォルダへ移る → 検索が終わる → そのフォルダから本を開いて
  Ctrl+Shift+↓ が通常フォルダの一覧順で移る (または端・対象外の案内が出る)。黙って何もしない結果にならない。
- 範囲外の入口 (メニューバーのお気に入り等) で検索を残したまま物理一覧へ移った状態の Ctrl+Shift+↑/↓ が、移動するか案内を出す。

## 11. 追補 3: サブ PC 確認 (ビルド a1ebaa2ae) の差分 3 件 (2026-11-11)

観測はサブ PC 担当 (統括セッション経由、依頼 34 第 1 部)。入口 9 つのうち 7 つと手順 3〜5 は期待どおり。違った 3 点への対応を決める。

### 11.1 メニューバーのお気に入り・本棚もそろえる (利用者決定 2026-11-11)

- 観測: メニューバーの「お気に入り」から開くと、Ctrl+R・Ctrl+S・Ctrl+T の検索中 3 回とも検索が閉じず、擬似子として表示された。
- 決定: **メニューバーのお気に入りとメニューバーの本棚も、ツールバー・アドレス欄・フォルダペインと同じく、移動先を開けた時点で
  検索を終える (失敗・取消・stale では結果と選択を残す)**。検索なしの挙動は変えない。
- お気に入りは、§9 のツールバーと同じ経路・同じ受付判定に乗せる。メニューバー・キー (Ctrl+1〜 等)・リング・ゲームパッドの
  お気に入り一覧など、お気に入りの他の利用者入口も同じ形にそろえる (同じ helper を通す)。本棚は §2.1 の場所ジャンプ
  `LocationJumpTarget::BooksRoot` と同じ値で運ぶ。
- 実装時に、**利用者の操作で物理の場所へ移るのに検索を残したまま (擬似子) で開く入口が他に残っていないか**をコードで全列挙し、
  表に残す。検索結果の中を開く (結果の本・フォルダへ降りる、Everything の PhysicalChild、検索内の ← / 親へ) は検索の一部なので対象外。
- 追加判断 (設計担当・利用者): native backdrop の早期 return は detached 述語・viewport lifecycle・registry transition ではなく
  main frame の navigation consumer なので凍結対象外。§11 に含める。Favorite だけの arm 追加で済ませず、gamepad_nav の全 variant を
  棚卸しし、LocationJump の既存取りこぼしを含め共通 dispatcher で処理する。検索なしの旧 loader と steps 後処理は維持する。
  各 variant の処理と、直接 producer がないものの根拠は §8.11 に記録し、無言の型破棄を残さない。

### 11.2 検索中のフォルダペインの Enter が一覧へ渡る

- 観測: 検索中、フォルダペインでフォルダを選んで Enter を押すと、ペインのフォルダではなく一覧で選ばれている結果の本が開いた。
  検索なしではペインのフォルダへ移れる。クリックでは移れる。
- 方針: Enter の受け手 (フォーカスの所有) の決め方を調べ、検索中でもペインにフォーカスがあるときは Enter をペインが消費し、
  一覧の「開く」に渡さないようにする。検索の有無で受け手が変わる分岐があるなら、その分岐が根本原因として直す
  (フォーカスの判定を 1 か所に寄せる)。実描画→キー入力の handler-level テストで、検索 4 種 × ペインの Enter がペインの
  フォルダへ移り、一覧の結果が開かないことを確かめる。

### 11.3 存在しないドライブ (`Q:\`) で通知が出ない

- 観測: 検索中にアドレス欄へ存在しないドライブ `Q:\` を入れると、結果と選択は残ったが通知は見えなかった。
  存在しないパスは検索の有無に関係なく親フォルダへ移る既存の動き。
- 方針: 通知が出ない経路 (無言の早期 return) をコードとテストで特定し、移れないときは通知を出す。検索していないときに
  `Q:\` を入れた場合の既存の動き (リリース済み) も確かめ、退行させない。検索なしで既に通知が出ているならその文言・経路に揃え、
  検索なしでも無言なら同じく通知を出す (無言の失敗は残さない)。

### 11.4 テスト

- 4 種の検索 × メニューバーのお気に入り・本棚 (と、そろえた他のお気に入り入口) で、採用後に検索が閉じ、← で検索前へ戻る。
  失敗では結果が残る。検索なしでは今と同じ。
- 4 種の検索でペインにフォーカスがある状態の Enter が、ペインのフォルダへ移る (一覧の結果を開かない)。検索なしの回帰。
- 検索あり / なしでアドレス欄に `Q:\` (存在しないドライブ) を入れると、移動せず通知が出る (検索ありなら結果と選択が残る)。
