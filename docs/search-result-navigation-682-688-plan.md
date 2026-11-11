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
   - Everything 取得中の main modal など既存の受付制限は、今の入口が持つ判定のまま (足しも外しもしない。
     既存 modal が location 入力を止めていないなら、実装者は報告して止まる)。
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
   - 場所▼ の変更は有効条件と確定先の差し替えだけにする (§1.756 が同じファイルの 履歴▼ を触るため、周辺を整形しない)。
5. 検索中のジャンプ後の「戻る (←)」は、右クリック「フォルダに移動」と同じく検索前の場所へ戻る (ExitTemporaryView の既存契約)。
   検索結果は履歴に積まない (§1.689 は保留のまま)。

**範囲外 (今回変えない)**: お気に入り (キー・リング・メニューバー・ツールバー・ゲームパッドのお気に入りリスト)、
アドレス欄、フォルダペイン。これらは今も検索中に動く (拒否されていない) ので >>717 の不便には当たらず、
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
   - surface が `Search(Global)` で `global_search.active` かつ `accepted` が Everything → `Everything { run }` (Root でも PhysicalChild でも)
   - それ以外 (Metadata・favsearch・tag・smart・評価一覧など) → `None`
   capture と position は同じ関数を使う (1 つの意味に 1 つの書き手)。
2. **取得 (capture)**: 既存の 5 か所の呼び出しのまま。条件の `surface == Folder` を `current_list_order_scope()` が
   `Some` に置き換え、その値を snapshot の `scope` に入れる。Collection / Rating owner の除外はそのまま。
   列の中身は今と同じ (`current_grid_order()` 順の Folder / Zip / Pdf / ConvertibleArchive 行、path 重複なし)。
   - Everything Root から開いた本: 結果一覧の表示順 (並べ替え・★/タグ/ファセット/Ctrl+F の絞り込み後) で取る。
   - Everything の実フォルダ行の中 (PhysicalChild の物理一覧) から開いた本: その物理一覧の順で、scope は同じ `Everything { run }`。
     通常フォルダでの「その一覧から開いた本の順」と同じ規則。
3. **位置 (position)**: `snapshot.scope == current_list_order_scope()` を足す。表示先 (MainWindow / Fullscreen)、
   detached でないこと、`effective_folder()` からの 2 段の位置の求め方は今のまま。
   - 帰結: 検索を閉じた後・別の検索を実行した後 (run が変わる)・Ctrl+G の本・通常フォルダでは Everything の列は使えない。
     Everything の本の中では、通常フォルダで取った列は使えない。どちらも「一覧から開いた本で使えます」の通知。
   - 列は 1 つだけ持つ (scope ごとに持たない)。Everything から本を開くと、それまでの通常フォルダの列は置き換わる。
     状態を増やさないための割り切りで、次に通常フォルダから本を開けば取り直される。
4. **検索結果が更新されたときの順序**: 列は凍結 (通常フォルダと同じ)。
   - 結果一覧へ戻って並べ替え・絞り込みを変えても、次にその一覧から本を開くまでは前の順を使う。
     本の中に居る間は再投影が起きない (Root でだけ行う) ので、本を開いている間に列と画面がずれることはない。
   - 再検索 (送信) は run が変わるので、古い列は位置が求まらず使えない。追従・再取得の処理は作らない。
   - 移動のたびに Everything へ再問い合わせしない。
5. **移動後の検索側の位置を、成功採用の境界で揃える**。`accept_everything_physical_child(path)` を次の規則にする
   (呼ばれる場所は今のまま: 実フォルダ install・ZIP/PDF/変換書庫の accepted closure・direct PDF の採用)。
   - Root: 今のまま (行なら PhysicalChild へ)。
   - PhysicalChild で、`path` が **dataset の行** であり、かつ `logical_source` 自身でも
     その物理子孫 (`path_key` の成分境界で比較) でもないとき → `logical_source = return_anchor = path` に置き換える。`route` は保持。
   - それ以外 (行でない・現在の本の中) → 今のまま変えない。
   これで一覧順で次の行へ移った後の「← 結果へ戻る」・§1.664 の通常 close・BS の親は、移動後の本を基準にする
   (結果へ戻ると移動後の本が選択される)。位置を index として持たない (§1.444 第 3 版の方針) ので、取消・失敗・
   後から採用される ZIP/PDF・hydration に対する受け渡しは要らない。
   - typed な由来 (「一覧順の移動だった」) を採用まで運ぶ案は採らない (§4)。この規則は一覧順以外の移動にも効く:
     検索中にアドレス欄・フォルダペイン・お気に入り等で **別の結果行** へ移った場合も、帰路の基準がその行になる。
     今は古い本が帰路の基準のまま残るので、これは改善として扱う (挙動の変化として文書・テストに残す)。
   - 結果行が別の結果行の中にある場合 (Everything が `A` と `A\B` の両方を返した) は、`A` の中から `A\B` へ移っても
     基準は `A` のまま (今の「本の中の移動」と同じ)。一覧順で `A` → `A\B` へ移った場合も同じで、結果へ戻ると `A` が選択される。
     まれな組み合わせとして割り切る。
6. **入力と gate**: `FsListOrderPrev/Next` の既存 handler・native 動画・gate (modal・準備中 lock) は変えない。
   Ctrl+↑↓・Ctrl+PageUp/Down の検索中の拒否も変えない。新しいリング・ゲームパッド割り当ては足さない (§1.444 と同じ)。
7. **着地**: `FolderNavMode::ListOrder` の既存経路のまま (検索を閉じない)。実装者は `apply_folder_nav_result` と
   `start_folder_nav` に、`global_search.active` 中に ListOrder を拒否・別処理する分岐が無いことを確かめ、
   あれば報告する。Folder 行から中の書庫へ着地する場合 (§1.444 §3.4 の 5-2) は、Folder 行の採用で基準が置き換わり、
   続く書庫の open はその子孫なので基準は変わらないことをテストで確かめる。
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
- まれな失敗 (scan 中のディスク異常等) は既定の割り切り (ログ + 通知)。新しい回復処理を足さない。

## 4. 検討した単純化 (CLAUDE.md「設計の簡素化」)

- **§1.688 を「gate を外すだけ」にしない**: 外すと検索の上にドライブ一覧が載る・検索が擬似的な子として残る
  (下流が検索を閉じない)。採らない。
- **「検索を閉じて元の場所へ戻してから再ジャンプ」の二段処理にしない**: 中間の復元 worker と組み合わせが増え、
  失敗時に結果が先に消える。採らない。既存の ExitTemporaryView の成功時 dismiss に一本化する。
- **新しい pending を足さない**: 既存の明示ジャンプの scan・`adopt_synthetic_surface`・評価一覧/閲覧履歴の採用に乗せる。
- **§1.682 で一覧順の由来を採用まで運ばない**: 運ぶと load の各経路 (実フォルダ・ZIP/PDF・変換書庫・direct PDF・hydration) に
  配管が要る (§1.444 第 2 版が REJECT された理由と同じ)。採用された path と dataset から導く規則 (§2.2-5) にした。
  引き換えに一覧順以外の移動でも帰路の基準が更新されるが、改善側の変化と判断した。
- **列を scope ごとに複数持たない**: 1 つに置き換えるだけにした。Everything から本を開くと通常フォルダの列は失われる。
- **検索結果の更新に列を追従させない**: 凍結。再検索は run の不一致で自然に使えなくなる。
- **お気に入り・アドレス欄・フォルダペインまで揃えない**: リリース済みの挙動が変わるので相談対象 (§2.1 範囲外)。

## 5. テスト (handler-level と状態遷移)

§1.688 (各ケースで Metadata / Everything / favsearch / tag の 4 種):
- キー・リング (`apply_ring_action`)・マウスボタン・ゲームパッドのリング・ゲームパッド場所リスト・場所▼ の各入口から、
  デスクトップ (実フォルダ)・ドライブ文字・ドライブ一覧・閲覧履歴・本棚・評価一覧へ移ると、採用後に検索が閉じ、
  移動先が表示され、← で検索前の場所へ戻れる (履歴が記録される)。入口が違っても同じ結果になる。
- 存在しない / scan 失敗の行き先: 検索 active・結果の行・選択が残り、通知が出る。
- ジャンプの scan 中に検索の完了が届く場合と、採用後に届く場合。採用後は一覧を上書きしない。
- ★固定中は今の拒否のまま。検索外では今と同じ `AddressBarNav` (回帰)。
- fullscreen (検索結果から開いた画像・本) からの場所ジャンプ: fullscreen を閉じて同じ規則。

§1.682:
- Everything Root から本を開くと scope=Everything{run} の列が取れる。並べ替え (名前・日付の昇降順)・詳細表示・
  ★/タグ/ファセット/Ctrl+F 絞り込みの順と一致し、画像・動画行は入らない。複数ドライブの本と単体ファイルの混在。
- Metadata・favsearch・tag・評価一覧などでは取らず、既存の列も変えない (`list_order_capture_excluded_surfaces_*` を
  `Search(Global)` の Everything / Metadata に分けて更新)。
- PhysicalChild の本で位置が出て、前後の行へ移れる (ZIP・PDF・EPUB・画像フォルダ本・変換書庫・実フォルダ)。両端の通知。
- 移動後に `logical_source` / `return_anchor` が移動先へ置き換わり、Results 帰還で移動先が選択される。§1.664 の
  通常 close も結果へ戻る。BS の親も移動先基準。
- 行でない子孫・現在の本の中の移動では置き換わらない。`A` と `A\B` の両方が行の場合。Folder 行から中の書庫への着地。
- 再検索 (run 変更) 後・検索 close 後・通常フォルダでは Everything の列で位置が出ない。逆に Everything の本では通常フォルダの列が使えない。
- 移動の取消・失敗で基準が変わらない。F12 linked / detached・★固定で使えない (既存条件)。
- 通常フォルダの一覧順移動の既存テストがすべて通る。

## 6. 文書

- [一覧順ナビゲーションの設計](list-order-book-navigation-plan.md): §3.3・§3.4・§5・§6 に scope と Everything の扱いを追記。
- [Everything 検索の設計](everything-search-plan.md) §7.1 と [検索アーキテクチャ](search-architecture.md) §1.2.1・§7.4:
  PhysicalChild の基準の置き換え規則、場所ジャンプの成功時終了。
- [keymap-spec](keymap-spec.md): 場所ジャンプの検索中の扱いの記述があれば更新。
- マニュアル: `fullscreen.html#list-order-navigation`・`search.html` (Everything 結果の本・検索中の場所ジャンプ)・
  `gamepad.html` (場所リスト)・`shortcuts.html`・他に「検索中は場所ジャンプを使用できません」等の記述があれば更新。
  製品ページ `index.html` の「一覧順で本を巡回」カード (150 字以内、`python scripts/check_site_cards.py`)。
- `docs/next-release-backlog.md` §1.682・§1.688 の状態。
