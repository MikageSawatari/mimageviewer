# 書庫内の本を開く・閉じる (§1.675 + §1.676) 設計

状態: **設計案 (2026-11-08)**。設計担当 ClaudeCode Opus 5.5。ソース調査と下書きは Codex GPT-6.1 Sol / high
(読み取り専用、`bec01bf11`)。利用者の採択・独立設計レビューの結果は §9 に追記する。

報告と制約の正本は [バックログ §1.675](next-release-backlog.md) と §1.676。関連: [仮想フォルダ](virtual-folders.md)
の「ネスト ZIP のツリーナビ」「§2.3」「退出ルーティング」、[内部書庫ツリー設計](nested-zip-tree-plan.md) §3.3・§6、
[detached リワーク計画 §2](detached-rework-plan.md)。§1.678 (子の本だけの ZIP へ Ctrl+↓) はこの設計の対象外。

## 1. 何を直すか

- §1.675: 「開いたとき、ページを表示」が有効でも、書庫内フォルダ・入れ子書庫 (`GridItem::ZipDir`) を
  ダブルクリック / Enter / パッドで選ぶと、常にページ一覧で止まる。3 つの入口が `zip_nav_enter` を呼び、
  同関数が `StartupListIntent::ExplicitList` 固定で階層を表示するため。
- §1.676: 書庫内の A / B 一覧から A を開いて閉じると、外側の実フォルダまで戻る。
  `resolve_close_return_navigation` の最後の分岐が `effective_folder().parent()` を返し、ZIP 内の階層を見ないため。

## 2. 決めた挙動

### 2.1 開く (§1.675)

- main の一覧で `ZipDir` セルを利用者が選んだとき (ダブルクリック / Enter / パッド決定。**1 つの共通入口**
  `activate_main_zipdir` を通す)、「ページを表示」の実効値が ON なら、子の階層に入り (既存の冗長ラッパー
  collapse を適用)、その階層に直接表示できるページがあれば本としてページを開く。
- 分類 (案 A): 内部フォルダも入れ子書庫も「ページを表示」だけに従う。どちらも書庫内の本 scope
  (`BookScope::Archive`) で、見開き設定の本キーや Ctrl+↑↓ の DFS も同じ単位で扱っているため。
  「画像のみフォルダは本として扱う」は実フォルダの分類設定のままとし、書庫内には効かせない。
  案 B を採る場合も、判定は 1 つの述語 (`is_archive` を見る) に閉じる。
- 直接表示できるページの判定は、ファイル種類・★・本の構成を適用した後の、その階層直下のページで行う。
  直下にページが無い分岐 (子の本だけ) は今までどおり一覧に留まる。最初の子へ潜る DFS・フラット化はしない。
- 開く位置: 内部本ごとの読書位置は今は保存されていない (`book_resume_recording_folder` が非 root を除外)。
  **今回は内部本を先頭 (構成適用後の最初の表示ページ) から開く**。内部本ごとの「続きから」は、読書位置の
  保存先を増やす永続化契約の拡張なので、別の単位 (§1.675 追補 / 新規番号) として設計・レビューする
  (利用者の採択待ち、§9)。外側書庫の root の読書位置を内部本へ流用しない。
- 直接表示しない入口は今の意図を保つ: Backspace / `zip_nav_back`、アドレス・パンくず、履歴 Back/Forward、
  refresh、ソート・ピンの再 materialize、★一覧の ZipDir、ブックマークの prefix 復元、Required leaf、
  コレクション、一覧の Ctrl+↑↓、フルスクリーン DFS、ファイル名スタック、★固定。
- 複数ウィンドウモード: **今回は ZipDir の選択を main の一覧移動のまま据え置く** (main 内のフルスクリーンへは
  送らない)。既存の detached descriptor は特定ページ (Required leaf) しか表せず、内部本の scope を渡すには
  descriptor の拡張が要る (概算 2〜4 人日)。後続の単位にする (利用者の採択待ち、§9)。

### 2.2 閉じる (§1.676)

- main (と main の bundle を映す linked F12) で、「ページを表示」の実効値が ON のときの通常の閉じる
  (Esc / × / Enter の close 割り当て / 右クリック → `handle_fullscreen_close_request`) は、
  **利用者がその本を選んだ書庫内の一覧**があればそこへ戻る。メモリ上のツリーから一覧を作り直し
  (再列挙しない)、読んでいた本のセルを選び (`zip_nav_back` と同じ前方一致規則)、選んだときの位置
  (セルが画面上端から何行目か) に戻す。
- 対象: §1.675 の直接表示、今の「内部のページ一覧からページを手動で開く」経路、深い階層、collapse 後の
  実効 prefix、同じ一覧の下でのフルスクリーン Ctrl+↑↓ (A → B に移ったら B を選ぶ)。
- 書庫内の一覧を利用者が選んでいない場合 (外側書庫を直接ページ表示して root ページから DFS で内部本へ、
  実フォルダからの Ctrl+↑↓ で書庫へ、ブックマーク・Required leaf で深い階層へ直接) は今の帰路のまま。
- 変えないもの: フルスクリーンの Backspace (今の本のページ一覧 L2 へ)、「ページを表示」OFF (L2 へ)、
  独立ビューアの close (§1.670 A: main を変えない)、同じ書庫を読む兄弟ビューアの状態。
- 帰路の優先順位: **記録済みの書庫内一覧を最優先**にし、その後に既存の専用帰路 (Everything 結果、
  コレクション、ブックマーク、閲覧履歴、★一覧、サブフォルダ展開、スマートフォルダ) と物理親が今の順で続く。
  書庫内一覧の記録は、専用帰路で書庫を開いた**後に**利用者が書庫内で本を選んだときにしか作られないので、
  「本を選んだ一覧へ戻る」がより新しい事実である。内部一覧へ戻った後は、Backspace で書庫の root、
  さらに外側の既存帰路 (コレクションの物理子・実親など) へ今どおり戻れることをテストで確かめる。
  内部一覧を選んでいない場合の専用帰路の挙動は変わらない。

## 3. 所有: 「どの一覧から本を選んだか」は `ZipNavState` の階層が持つ

既存の `ZipNavLevel { display, raw }` (実効 prefix スタックの 1 段) に、排他的な値を 1 つ足す。

```rust
enum ZipListChoice {
    /// 生成しただけ (enter / restore_prefix / DFS / refresh)。一覧を見た証拠にならない。
    Unseen,
    /// 利用者がこの階層の一覧でセルを選んだ。帰還時の位置もこの値が持つ。
    Chosen { rows_above: Option<u32> },
}
```

- **producer は共通入口 `activate_main_zipdir` だけ**。一覧でセルを選んだこと自体が、その一覧を見た証拠になる。
  出発階層 (現在の stack 末尾) を `Chosen { rows_above }` にしてから子へ入る。`rows_above` は既存の
  `ListCursorHint` と同じ「選択セルが表示上端から何行目か」(`selected_rows_below_view_top`)。
- 内部のページ一覧 (L2) からページを手動で開く経路は、新しい記録を作らない。その本を選んだ親階層の
  `Chosen` が残っているので、それを帰路に使う。
- `startup_list_restore` (保存される「最後に明示的に開いた一覧」) は使わない・変えない。プロセスをまたぐ保存値で、
  今のツリーとの相関も DFS で作った階層の未表示性も示せないため。
- 帰路の解決: close の時点で、**現在の本を含む最も深い `Chosen` の厳密な祖先階層**を探す。その階層まで stack を
  pop し、`ExplicitList` で表示し、読んでいる本 (現在の実効 prefix) からセルを選び、`set_row_anchor(rows_above)`
  で位置を戻す。`rows_above` が無ければ選択セルを reveal する。
- 配送: 既存の `FullscreenCloseContinuation` / `CloseReturnEffect::Navigation` に、typed な nav
  `AddressBarNav::ZipListReturn(plan)` を載せる。plan は owner から導いた一回限りの値で、
  別の来歴 owner ではない (中身: 帰還する階層の深さと display prefix、読んでいた本の prefix、`rows_above`、
  ツリーの identity)。通常の update、`handle_fullscreen_close_request_immediate`、in-window 静止画の
  early-return は、すべて既存の `apply_fullscreen_close_nav_immediate` の新しい分岐へ集める。
- 消費時の照合: plan が現在の main の同じツリー (`Arc` の同一性) と、まだ `Chosen` の階層を指すこと。
  外れていたら既存の物理親帰路に戻す (ログを出す)。

### 3.1 生成・失効

| 出来事 | 扱い |
| --- | --- |
| 共通入口でセルを選ぶ | 出発階層を `Chosen` に更新 (同じ階層で別の本を選び直せば上書き) |
| 同じ書庫内の本の切替 (フルスクリーン DFS) | DFS は stack を pop / push するだけなので、共通の祖先階層の値はそのまま残る。新しく push した階層は `Unseen` |
| Backspace で階層を戻る | pop するだけ。親の `Chosen` は残る |
| 別の書庫・フォルダの採用 | `zip_nav` ごと破棄される (既存) |
| refresh (ツリーの作り直し) | 記録を捨てる (全階層 `Unseen`)。書庫の中身が読書中に変わるのはまれなので、今の帰路へ戻る割り切り |
| 同じ階層の再 materialize (ソート・ピン) | stack を変えないので維持 |
| 履歴 Back/Forward・ブックマーク・Required leaf | `restore_prefix` で作った祖先は `Unseen` |
| ★固定 snapshot | 既存の `saved_zip_nav` 退避・復帰に値ごと同行。固定中は帰路を発行しない |
| 独立ビューアの build / fork | **複製する `ZipNavState` の記録を `Unseen` にする** (§1.670 A と同じく、main への帰還権限を独立 bundle に作らない) |
| main admission・Remote 取得・終了 | 既存の close return 失効入口 (`retire_close_returns_for_main_admission` 等) がそのまま配送中の plan を捨てる |

## 4. 開く処理の接続 (§1.675)

- 共通入口は、出発階層の記録 → 現行の★一時解除 (`maybe_suppress_rating_filter_for_opened_zip_book`) →
  clone した nav で子へ入る → 分類の述語 → 一覧なら `ExplicitList`、本として開くなら `PageContinuation` で
  階層を install する。現在の `zip_nav_enter` は一覧移動用 helper として残し、既存の他の caller は変えない。
- install 直後に先頭 idx を選んで開かない。階層 install の後は既存の `ContinuationOwner::IndexOnly`
  (ページ identity の index 準備) が動くので、その完了動作に型付きの値を持たせる:
  `IndexOnlyAfterPrepare::{None, OpenArchiveBook(DeferredFsReopen)}`。新しい App の pending は作らない。
- `DeferredFsTarget` に `CurrentArchiveBook { scope }` を足し、leaf 指定 (Required / Preferred) と区別する。
  完了時に scope・context・items 世代が一致することを確かめてから `open_deferred_fullscreen_after_enumerate`
  に入り、既存の本の構成待ち・表示許可を通って先頭の表示ページを開く (`from_explicit_open = true`、
  `HistoryTrigger::UserChosen`、今回は `resume_to_last_page = false`)。
- 構成適用後に直接表示ページが無かった場合は、型付きの結果で**その階層の一覧表示**に終える。子の本を探さない。
  §1.678 の `NoPlayableItem` と独立窓の終端の意味は変えない。
- 取消・失敗・古い完了: 既存の sidecar / 構成 owner が要求を捨てる。キャンセル後に勝手に開かない。
- Enter で開いた直後に同じ Enter が close に流れない既存の抑制 (`fs_suppress_enter_close_until_release`) を適用する。

## 5. 簡素化の確認

- 3 つの入口の予約を、1 つの共通入口と 1 つの型付き値にする。
- 帰路は「一覧でセルを選んだ」という 1 種類の事実だけから作る。一覧採用の通知や保存記録との連動を足さない。
- refresh・履歴・独立化では記録を捨てて今の帰路へ戻す。記録の移し替え規則を増やさない (DFS で共通祖先が残るのは stack の性質で、移し替えではない)。
- モーダルは不要: 内部階層の列はメモリ上のツリーから作り、非同期は既存の index 準備・構成待ちとその取消を使う。
- まれな失敗は既定の割り切り (ログ + 通知)。再試行・回復用の状態は作らない。

## 6. 永続化

- 永続データの形式・意味は変えない (`startup_list_restore` V1、読書位置とも)。帰路の記録は実行時だけ。
- 内部本ごとの読書位置を足す場合 (§9 の採択次第) は別単位で、永続化の判断表に沿って扱う。

## 7. detached §2 との関係

- 触れる見込み: 共通 close dispatcher (`close_return.rs`)、linked viewport の終端からの帰路捕捉、
  registry の build / fork で複製する `ZipNavState` の記録の消去。
- 変えない: HWND の同定、viewport identity、placement、再生成、detached の述語、時間窓。
  `zip_nav` は今までどおり bundle の所有。
- 「どの context が、どの一覧への帰還権限を持つか」を producer 境界 (一覧でセルを選んだ時点・独立 bundle を
  作る時点) で決める構造修正であり、close 後に main を直す guard や兄弟の再読み込みではない。
  実装後に設計担当と独立レビューの双方で合意を取り、[detached 計画 §11](detached-rework-plan.md) に記録する。

## 8. テスト (headless)

既存の `AppTestEnv`、`startup_restore.rs` の `write_zip`/`settle_book`、`ctrl_down_close_1678.rs`、
`everything_close_return.rs` の fixture を使う。実アプリ・OS の窓・キー入力は使わない。

- `section1675_zipdir_activation_routes_all_inputs`: ダブルクリック handler・Enter・パッドが同じ共通入口を 1 回通る
  (内部フォルダ / 入れ子 ZIP)。
- `section1675_zipdir_setting_matrix`: 「ページを表示」ON/OFF × フォルダ / 書庫。直接ページの有無、子への DFS なし。
- `section1675_branch_and_collapsed_wrapper`: 子の本だけの分岐は一覧、単一・多段ラッパーの collapse 後に正しい本。
- `section1675_projection_gates_open`: 本の構成 (順序変更・非表示)、ファイル種類の除外、★。表示許可の前に開かない。
  構成で直下ページが 0 なら一覧に終わる。
- `section1675_converted_zipdir_uses_logical_source`: 変換 cache (RAR/7z を source とする) で論理 source と backing を区別。
- `section1675_stale_and_cancel_do_not_open`: index 準備中に別の階層・別の書庫へ移る、取消、古い完了で開かない。
- `section1675_multiwindow_zipdir_keeps_main_list`: 複数ウィンドウモードでは main の一覧移動、main のフルスクリーンなし。
- `section1676_direct_and_manual_open_return`: 直接表示 / L2 から手動でページを開く。Esc・×・Enter close・右クリックの
  共通 handler で A/B 一覧、A を選択、位置が戻る。ZIP と変換 RAR。
- `section1676_deep_and_collapsed_return`: 多段・collapse。literal なセルを選び、見ていない中間階層へ戻らない。
- `section1676_dfs_selects_current_sibling`: A/B 一覧 → A → フルスクリーン DFS → B → close で B を選択。
- `section1676_unseen_parent_keeps_old_return`: 外側の直接表示 → DFS、実フォルダからの Ctrl+↓、ブックマーク。今の帰路。
- `section1676_backspace_and_auto_off_keep_l2`: フルスクリーン Backspace と「ページを表示」OFF は今の本の L2。
- `section1676_history_refresh_and_stale`: 履歴移動・refresh 後は記録なし。古い plan から開かない・戻らない。
- `section1676_special_return_precedence_and_back_out`: Everything・コレクション・閲覧履歴・ブックマーク経由で書庫を開き、
  内部で本を選んだ場合は内部一覧へ。そこから Backspace で root → 外側の既存帰路へ戻れる。内部で選ばない場合は専用帰路のまま。
- `section1676_linked_f12_returns_main_level`: linked F12 の close で main の bundle に 1 回だけ戻る。
- `section1676_independent_close_never_mutates_main`: 独立ビューア (fork 含む) の close で main の一覧・選択・位置が不変。
- `section1676_same_archive_sibling_is_unchanged`: 同じ書庫を読む兄弟の items・世代・キュー・取消が不変。
- 既存回帰: `zip_nav_back_selects_book_cell_left_from`、`zip_nav_ctrl_updown_*`、`section1335_*`、`section1328_*`、
  `collection_zip_details_auto_close`、`detached_close_keeps_main_results_in_both_window_modes`、`section1678_*`。
- 変換 RAR は変換済み cache を使う headless 検証であり、実 RAR の変換そのものの検証とは分けて書く。

## 9. 採択・レビュー

- 利用者の採択待ち: (1) 分類は案 A でよいか、(2) Esc の帰り先が内部一覧に変わること、
  (3) 内部本ごとの「続きから」を今回に含めるか、(4) 複数ウィンドウモードを後続にするか。
- 独立設計レビュー (Codex GPT-6.1 Sol / xhigh): 未実施。
