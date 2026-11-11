# 検索結果からの一覧順移動 (§1.682) と、検索中の場所ジャンプ (§1.688)

2026-11-11 第 1 版 (設計 = ClaudeCode Opus 5.5)。設計の独立レビュー = Codex GPT-6.1 Sol / xhigh、
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

1. **受付を 1 つの helper にまとめる**。`location_navigation_blocked() -> bool` を
   `location_jump_admission() -> Option<LocationJumpMode>` に置き換える。
   - ★固定中: 従来どおり通知して `None`。
   - 検索 3 owner のどれかが active: `Some(LocationJumpMode::ExitTemporaryView)`。
   - それ以外: `Some(LocationJumpMode::Plain)`。
   - Everything 取得中の main modal など既存の受付制限は、今の入口が持つ判定のまま。
   - **dispatch の時点でも取得中 modal を確かめる** (§7 P2)。キーの navigation 値はフレームの前半で作られ、
     同じフレームの検索バー描画で Everything の送信が始まった後、フレーム末尾で dispatch される
     (`src/app.rs` 約 100515・101050・101130)。下流の `document_open_modal_admission_blocked` は Everything を含まず、
     `adopt_synthetic_surface` にも判定が無い。そこで `begin_exit_temporary_view_jump` の先頭で、主一覧の open / navigation が
     共有する Everything 取得中の target-context admission を再判定し、取得中なら何もしない (案内は既存の取得中表示に任せる)。
2. **行き先を型で表す**。`enum LocationJumpTarget { Physical(PathBuf), DriveList, ReadingHistory, BooksRoot, Rating(u8) }`。
   既存の各 helper (`apply_ring_drive_letter` 等、`quick_location_nav`) は行き先の解決 (存在確認・通知) までを今のまま行い、
   最後に共通の `fn location_jump_nav(&mut self, target, mode, source) -> Option<AddressBarNav>` へ渡す。
   fullscreen を閉じる・`bump_input_seq`・repaint もこの helper に集める (今は各 helper に重複している)。
   - `Plain`: 今と同じ値を返す (`Direct(path, ExplicitList)` / `DriveList(None)` / `ReadingHistory(Menu)` / `BooksRoot`、
     評価一覧は今と同じ `enter_rating_view`)。**検索外の挙動は変えない**。
   - `ExitTemporaryView`: 新しい `AddressBarNav::ExitTemporaryViewJump(LocationJumpTarget)` を返す。
3. **dispatch も 1 か所**。`AddressBarNav::ExitTemporaryViewJump(target)` を受ける全 match
   (`update_phases.rs` の input/address・context・grid の 3 か所、`apply_fullscreen_close_nav_immediate` 等の即時 dispatcher)
   は同じ `fn begin_exit_temporary_view_jump(&mut self, target)` を呼ぶ。中身は既存の前例に乗せる:
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

**範囲外 (今回変えない)**: お気に入り (キー・リング・メニューバー・ツールバー・ゲームパッドのお気に入りリスト)、
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
   - `Everything` scope: **Root (結果一覧そのもの) のときだけ** 取る。並べ替え・★/タグ/ファセット/Ctrl+F 絞り込み後の表示順。
     PhysicalChild の中 (実フォルダ行の物理一覧) から本を開いたときは取らず、前の列を残す (第 1 版から変更、§7 P1)。
3. **位置 (position)**: `snapshot.scope == current_list_order_scope()` を足す。表示先 (MainWindow / Fullscreen)・
   detached でないことの条件は今のまま。今の本の求め方は scope で分ける:
   - `Folder`: 今と同じ (`effective_folder()` の完全一致 → Folder 行の配下)。
   - `Everything`: **採用済みの `PhysicalChild.logical_source` と完全一致する entry** の位置。Root・一致なしなら `None`。
     検索 session が「どの結果行を開いているか」を既に持っているので、物理パスの祖先推定をしない。Everything が
     `A` と `A\B` の両方を返した場合や、内側の書庫も結果行である場合でも、位置は開いた行に一意に決まる。
     Folder 行を開いてその中の本を開いた状態から押すと、その Folder 行の前後の結果行へ移る。
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
     `enum EverythingChildAdmission { Descend, ListOrderRow { run: u64, row: PathBuf } }`。
     既定は `Descend` (今の動き)。`ListOrderRow` を作るのはこの 1 か所だけ。
   - 載せ方: 要求ごとに既に運ばれている search origin (`load_folder_with_scan_claimed` の install closure、
     `PhysicalHistoryTransition.global_search_origin`、direct PDF の採用、変換書庫の継続) に、一覧順の移動の要求だけが
     `ListOrderRow` を渡せるよう、引数または要求の field で通す。App の新しい pending・bool・「次の採用で使う」field は作らない。
     どの関数に引数を足すかは実装者がコードで決め、全経路を列挙して報告する。変換ダイアログの直接呼び出し
     (`src/ui_dialogs/archive_convert.rs` 1148) が一覧順の着地に関わるかも確かめる。
   - 採用 (`commit_staged_search_navigation_effects` → `advance_drilled_current_path` → `accept_everything_physical_child`) で:
     - `Descend`: 今と同じ (Root なら行へ、PhysicalChild なら変えない)。**一覧順以外の移動の挙動は変えない**。
     - `ListOrderRow { run, row }`: origin が current (既存の run・context・active 照合)、accepted の run が一致、
       採用された path が `row` と一致 (`path_eq`)、`row` が dataset の行、のすべてを満たすときだけ
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
- 場所キーの navigation 値を作った同じフレームで Everything の送信が始まると、ジャンプは実行されない (dispatch 時の再判定)。
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
