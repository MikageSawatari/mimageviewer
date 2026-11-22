# フォルダツリーの見える行だけを組み立てる (§1.827 ツリー仮想化)

状態: 設計 (実装前レビュー待ち)。対象ブランチ `tree-virtualize-827` (master ef847a1cc から)。

## 1. 観測と出どころ

- 報告者 (5ch >>833・857・858・867) の観測: 直下約 6,000・配下約 3 万フォルダの場所の下で画像を
  スクロールすると重くちらつく。ツリーを非表示にすると症状が出なくなる (報告者が確認)。
  報告者側の集計で `folder_pane_ms` 中央値が約 0.15 ms → 約 57 ms、`main_galleys` が約 95 → 約 7,100。
  原ログは受領していない。
- 開発側の計測: §2 (今の master、使い捨ての合成ツリー)。

## 2. 今の master の計測

計測はすべて開発側の使い捨てデータ。実行したのは portable-smoke のコピー (`--data-dir target\portable-smoke\data`) と、cargo test 内の headless App で、利用者のデータは使っていない。
合成ツリーは `scripts/tree827/New-LargeFolderTree.ps1` で作った。`book\A\02` の下に 6,000 フォルダ (各 5 子フォルダ、合計 36,000) を置き、そのうち 1 つを画像フォルダにした (重い場合)。軽い場合は `book\A\01` の下に 20 フォルダを置いた。どちらも画像は同じ 8 枚。

### 2.1 headless (debug profile、`src/app/folder_pane_perf827_tests.rs`、計測専用で master に入れない)

ROOT の 1 pass で `render_folder_pane` を呼び、50 pass の中央値を取った。

| 直下の行 | 組み立てた行 | galley | render_folder_pane | visible_rows() 1 回 |
| ---: | ---: | ---: | ---: | ---: |
| 100 | 106 | 111 | 4.5 ms | 0.22 ms |
| 1,000 | 1,006 | 1,011 | 48 ms | 2.2 ms |
| 3,000 | 3,006 | 3,011 | 166 ms | 7.0 ms |
| 6,000 | 6,006 | 6,011 | 294 ms | 13.4 ms |

組み立てる行と galley は論理行と同数で、時間も行数に比例する。`visible_rows()` は 1 フレームに 3 回呼ばれる。
tessellate は 0.14 ms、shape は 118 で、行数に関係なく小さかった。画面外の行の塗りは clip で落ちている。

### 2.2 実アプリ (portable-smoke、release、test-script の合成キー入力、2026-11-22 20:02〜20:09)

UI スレッドの CPU は 1 コアに対する割合。perf イベント `frame/root_outside` は、§3 の計装で今回足したもの。

| 条件 | 全画面スクロール中の main fps | UI スレッド | folder_pane_ms | App::update | accesskit の受け渡し (platform) | accesskit ノード | 初回の最大停止 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 重い・そのまま | 16 | 96% | 36.6 | 40.8 | 18.1 ms/フレーム | 42,830 | **45.6 s** |
| 重い・accesskit 無効 (計測用の切り替え) | 26 | 91% | 30.1 | 34.5 | 0 | — | なし |
| 重い・自作 UIA クライアントあり | 15 | 95% | 37.3 | 41.7 | 18.3 | 42,830 | **37.4 s** |
| 軽い・そのまま | 153 | 65% | 0.7 | 4.1 | 0.2 | 970 | なし |
| 軽い・accesskit 無効 | 162 | 60% | 0.6 | 4.2 | 0 | — | なし |
| 軽い・UIA クライアントあり | 156 | 61% | 0.7 | 4.1 | 0.2 | 970 | なし |

- **このメイン PC では、自作のクライアントを付けなくても accesskit が有効になった** (起動から 4 フレーム目。どのクライアントかは特定していない)。
- **初回の長い停止**: ツリーが最初に全行を出したフレームで、accesskit の更新 (42,830 ノード) を OS 側の adapter へ渡す区間 (`platform_ms`) が 45.6 s / 37.4 s かかった。その間、UI スレッドは CPU を 93% 使い続けていた (mIV の外の待ちではない)。利用者のメイン PC の停止 (update の外で 70 s / 118 s、統括のログ確認) と同じ形。
- 毎フレームの費用は、ツリー全行の UI 組み立て (約 30〜37 ms) と accesskit の受け渡し (約 18 ms) の 2 つ。tessellate (0.13 ms)、GPU の encode / submit / present (各 0.1〜0.3 ms)、頂点数 (約 8,000)、texture delta (スクロール中 0) は、重い場合も軽い場合も小さく、ほぼ同じだった。
- GPU の 3D 使用率 (システム全体) は、重い・そのまま・スクロール中で平均 19.6%。DWM の CPU は権限の都合で読めなかった。
- 結論: 遅さの原因は、全論理行の UI 組み立てと、行数に比例する accesskit ノードの 2 つ。描画量でも GPU でもない。仮想化は両方を可視行の数まで減らす。


## 3. 原因の構造 (コード)

`App::render_folder_pane` は main chrome の描画のたびに呼ばれ、全画面表示中も root viewport は
毎フレーム描画されるため、画像のスクロール中もフレームごとにツリー全体を処理する。

0. **accesskit のノードも全論理行**: egui は描いた部品ごとに accesskit のノードを作る。
   UIA のクライアントが接続していると (このメイン PC では常時)、全行ぶんのノード (6,000 行で約 4.3 万) を毎フレーム OS 側の adapter へ渡す。
   ノードが大きく変わったフレームでは、その処理が数十秒かかった (§2.2)。
1. **UI 構築が全論理行**: `render_folder_pane_tree_with` が `ScrollArea::show` の中で全 `rows` を
   回し、行ごとに `scope_builder`・`horizontal`・矢印 `Button`・名前 `Label` (truncate のため文字配置)・
   ツールチップ用の `path.display().to_string()`・ヒット情報を作る。画面外の行も egui の
   レイアウトを通る (クリップで描画が省かれるのは塗りだけ)。
2. **データ側の全件処理が 1 フレームに複数回**: `visible_rows()` は全論理行を DFS で作り直し、
   行ごとに `key_for` (小文字化した String)・`seen` への挿入・`path_eq` 2 回 (各 2 回の正規化) と
   `PathBuf` の複製を行う。1 フレームに `handle_folder_pane_keyboard` → `sync_folder_pane_state`
   → `reconcile_demand`、`render_folder_pane` → `sync_folder_pane_state` → `reconcile_demand`、
   描画本体の 3 回呼ばれる。描画では全パスを `Arc<[PathBuf]>` に複製し、前フレームと比較する。
   `reveal_active` / `reachable` は祖先ごとに子一覧を `path_eq` で線形に探す。

## 4. 方針

### 4.1 描画の仮想化 (中心)

`ScrollArea::show` を `ScrollArea::show_rows(ui, pitch - item_spacing.y, rows.len(), ..)` に替え、
可視範囲 (egui の計算は `floor(top / pitch)` 〜 `ceil(bottom / pitch) + 1`) の行だけを組み立てる。
行の中身 (`draw_folder_pane_row`)、path 単位の `scope_id`、ヒット情報の記録、右クリック・
非 primary の起動・D&D の行情報は、描画した行について今と同じ処理をする。

- **行の高さをその pass の範囲計算の前に確定する** (Sol 設計レビュー P1-1): 前回の実測 pitch を配置に使うと、
  pitch が変わった pass で自動スクロールの座標と行の位置がずれ、行の実高が小さくなった場合に下端まで描けない。
  そこで、行の部品が要る高さを、描く前にその pass のフォントと spacing から求める
  `folder_pane_row_height(ui) = max(FOLDER_PANE_ROW_HEIGHT, 矢印 (16pt strong の行高 + button_padding.y × 2),
  名前の行高, spinner (interact_size.y), "!" の行高)`。`draw_folder_pane_row` はこの高さを受け取り、
  `set_height` / `add_sized` / `allocate_exact_size` をすべてこの高さで行う。部品はこの高さに収まるように
  求めてあるので、行の割り当ては全行 (loading / error を含む) で同じ高さになる。
  `pitch = row_height + item_spacing.y` を、描画範囲・全体の高さ・reveal の矩形・保存する
  `FolderPaneScrollMemory::pitch` のすべてに使う (1 つの値)。24pt に切り詰めることはしない。
  描いた行の実高が `row_height` と違えば debug_assert で落とし、テストで固定する。
  `request_discard` による同一フレームの作り直しは使わない (§1.828 のパス番号に依存する入力の関門と
  組み合わせが増えるため)。
- **内容の原点**: `show_rows` の callback の Ui は描画範囲の先頭行が原点になる。内容の原点を
  `ui.max_rect().top() - range.start × pitch` とし、論理行 i の矩形は `原点 + i × pitch` から作る。
- **全体の高さ** は `rows.len() * pitch - spacing` で、スクロールバーの長さと位置は今と同じになる。

### 4.2 現在地への自動スクロールを論理行の位置から

今は現在地 (操作カーソル) の行の `Response` に `scroll_to_me` している。仮想化すると画面外の行は
`Response` を持たないので、**論理行の index から行の矩形を計算して `ui.scroll_to_rect(rect, Center)`**
する (`scroll_to_me` も内部では同じ `scroll_to_rect(self.rect)`)。発火条件 (`Requested`・paths の変化・
pitch の変化・`FollowingRefresh` の扱い・ポインタ操作で `Idle` に戻す) は今と同じ。`cursor_drawn` は
「論理行に現在地がある」の意味になる (今も全行を回すので同じ意味)。

### 4.2b 同じ pass で描画範囲の外へ出た行への AccessKit 要求 (Sol 設計レビュー P1-2)

今は、行の非 primary の起動 (AccessKit の Click 要求、または egui の focus を持つ部品への Enter / Space) を、
描画中の `resp.clicked() && !clicked_by(Primary) && !clicked_by(Secondary)` で受けている。仮想化すると、
同じ pass のポインタ結果 (`take_label_results` の矢印など) で上の枝が開き、要求先の行が描画範囲の外へ
押し出されたとき、その行の `Response` が作られず、要求が消える
(既存 `folder_pane_label_pointer_arrow_preserves_other_row_accesskit_activation` の契約)。

- 描画の前、`take_label_results` を適用した直後に、**前 pass の `label_display` (この pass の入力の関門が使う
  ものと同じ) の label / arrow の ID → path** を使って、この pass の AccessKit の `Click` 要求を裁定する。
  裁定の中身は今の描画中の分岐と同じ (label: ジェスチャの解除・ツリーへの focus・カーソル・起動、
  arrow: 論理行で開閉できる行なら開閉)。`label_pointer_activation_owned` による重複の抑止、
  `main_chrome_pointer_blocked` 系の条件、ポインタ結果 → AccessKit の適用順も今と同じにする。
- 裁定した ID はこの pass の集合に入れ、描画中の分岐はその ID を飛ばす (二重に起動しない)。描画中の分岐は、
  egui の focus を持つ描画中の行への Enter / Space (AccessKit の要求でない起動) のために残す。
- `ScrollIntoView` の要求も同じく前 pass の ID → path で受け、論理行の index から §4.2 と同じ形で
  スクロールする。egui の `Focus` 要求は描かれた部品にだけ効く (egui の仕様)。描画範囲の外へ出た行への
  `Focus` は効かないが、ツリーの操作は egui の focus を使わないので、操作の結果は変わらない。
- 前 pass の表示が無い・古い (入力の関門が `DisplayNotFresh` などで捨てる) ときは、その pass の
  AccessKit 要求も描画中の分岐だけで受ける (今と同じ)。

### 4.3 データ側の全件処理 (計測で決める)

仮想化した後も、毎フレームの全論理行の処理が残る (§3 の 2)。headless の debug では、
`visible_rows()` 1 回が描画全体の約 5% で、1 フレームに 3 回呼ばれる。release ではおよそ数 ms と見込む
(仮想化後に実測する)。意味を変えずに 1 回あたりの費用を下げる、次の 3 つを入れる。

1. **`push_visible_rows` の正規化を 1 回にする**: 現在地とカーソルの key を `visible_rows()` の入口で 1 回だけ
   作り、各行は自分の key (すでに作っている) と比べる。`path_eq(a, b)` は定義上
   `normalize_keep_drive(a) == normalize_keep_drive(b)` で、`key_for` も同じ関数なので、結果は同じ。
   `seen` への挿入は key の複製をやめ、移動にする。
2. **`reveal_active` / `reachable` の線形探索をやめる**: 祖先ごとに `cache.children.iter().any(path_eq)` で
   「表示する子に含まれるか」を探している。`CachedListing` に、表示する子の key の集合 `child_keys` を足し、
   `children` と同じ 1 つの構築関数 (worker 側の走査と test 用の seed) で作る。`membership` (隠しフォルダも
   含む全ディレクトリ) とは意味が違うので、代わりには使わない。
3. **描画のスクロール記録を、変わったときだけ作り直す**: 毎フレーム全パスを `Arc<[PathBuf]>` に複製してから
   比べていたのを、前回の記録と行のパスをその場で比べ、違うときだけ複製する。

採らないもの: `visible_rows()` の結果をフレームをまたいで持つ (§6)、`sync_folder_pane_state` の 2 回呼びを
1 回にまとめる (キー処理と描画の間で状態が変わり得るので、順序の検討が別に要る)。仮想化と上の 3 つの後に
全件処理がまだ目立つ場合は、数字を付けて報告し、別の段階で扱う。
仮想化後の計測には、定常状態に加えて「多数の枝を開いてから畳んだ状態 (論理行より cached node が多い。
`has_pending()` は全 node を走査する)」と「走査の完了結果を採用するフレーム (`poll_pending` が
`reconcile_demand` を追加で呼ぶ)」を入れる (Sol 設計レビュー P3-7)。


## 5. 仮想化しても変えない 5 つ

| 守るもの | 今の持ち主 | 仮想化後 |
| --- | --- | --- |
| §1.828 の入力の関門の順序 | `begin_folder_pane_label_input` (前パスの `label_display` を使う) → `take_label_results` → 描画 → `label_display` の公開 → `finish_folder_pane_presentation` | 順序と各段の条件は変えない。変わるのは `label_display.rows` に入る行が「描画した行」に限られることだけ。ポインタが当たり得るのは可視の行だけなので、ヒット判定の結果は同じ |
| 単 / ダブルクリック・右クリック・D&D のヒット情報と path 単位の ID | 行ごとの `scope_id(("folder_pane_row", normalize_keep_drive(path)))`、`FolderPaneLabelWidgets`、`DropGeometry` (可視域と交わる行だけ採用済み)、AccessKit の Click は描画中の `resp.clicked()` | 同じ ID・同じ矩形で記録する。描く行 ⊇ 可視域と交わる行 (§4.1 の範囲と全行同じ行高) なので、D&D の行一覧は今と同じ。AccessKit の Click / ScrollIntoView は前 pass の ID → path で描画前に裁定する (§4.2b)。貼り付けメニューは path と画面上の位置を自分で持つので、元の行が描かれなくても保たれる |
| 現在地への自動スクロール | 行の `Response::scroll_to_me` | 論理行 index の矩形へ `scroll_to_rect` (§4.2)。画面外の現在地も表示できる |
| キーボードで辿る全論理行 | `handle_tree_key` → `move_cursor` が `visible_rows()` (データ) を使う | 変えない (描画に依存しない) |
| 折りたたみ前後の位置 | `FolderPaneScrollMemory::restore` (先頭行の path と行内の位置を、新しい一覧の index × pitch へ移す) | 変えない。全体の高さが行数 × pitch なので、restore の座標系も今と同じ |

## 6. 状態の組み合わせを減らす検討

- `visible_rows()` の結果をフレームをまたいでキャッシュし、変更のたびに無効化する案は採らない。
  行の内容は nodes・展開集合・現在地・カーソル・ドライブ・走査状態に依存し、書き手が多い。
  無効化の漏れという新しい組み合わせを作るより、1 回あたりの費用を下げる (§4.3)。
- `request_discard` は使わない (§4.1)。
- 新しい状態は足さない。仮想化は「どの行を組み立てるか」を描画のその場で決めるだけで、
  行 pitch とスクロール位置の記録は既存の `FolderPaneScrollMemory` を使う。

## 7. 変わること・退行の確認

- 画面外の行は AccessKit のツリーに出なくなる。§2.2 の停止を解消する中心の効果でもある。
  mIV のツリー操作は独自のカーソル (`handle_tree_key`) で、AccessKit のノードには依存しない。
  **スクリーンリーダー等で画面外の行を読み上げ順に辿る使い方は、可視の範囲だけになる。これは既存の挙動の
  縮小なので、利用者の判断を得る (Sol 設計レビュー P1-3、統括経由で照会中)。** 全行のノードを残す案は、
  4 万ノードの受け渡しによる数十秒の停止と毎フレーム約 18 ms をそのまま残す。
- 本番では Tab による egui の focus 移動を無効にしている (`src/egui_focus_policy.rs`)。AccessKit の `Focus`
  要求などで行の部品が egui の focus を得たまま、その行が描画範囲の外へ出ると、egui は focus を外す
  (描かれなかった部品の focus は egui が外す)。ツリーの操作は egui の focus を使わないので、操作の結果は変わらない。
- 上の 2 点以外に、通常の操作が遅くなる・できなくなる変更は無い見込み。
- 細かな違い: ツールチップは描いた行にだけ付く (ポインタが当たるのは可視の行だけなので、見え方は同じ)。

## 8. detached・複数ウィンドウ

ツリーは root viewport だけで描画する (`render_folder_pane_tree_with` の先頭で ROOT 以外は戻る)。
detached の述語・viewport の選択や配送には触れない。

## 9. まれな失敗

対象外 (ディスク・DB に書かない描画の変更)。

## 10. テスト

新しい回帰テスト (すべて headless、使い捨てデータ):

1. **組み立てる行の上限**: 6,000 行の木で、`label_display.rows` が「表示域の高さ ÷ pitch + 3」以下であること。
   accesskit を有効にした pass で、出力の `accesskit_update` のノード数が行数に比例しない
   (例: 1,000 行と 6,000 行で同程度) こと。
2. **初回の pitch の下限**: スクロール記録が無い最初の pass でも、描いた行が表示域の下端まで届くこと。
3. **画面外の現在地への自動スクロール**: 6,000 行で `set_cursor(5000 行目)` → 数 pass 後にその行が描かれ、
   表示域の中央付近にあること。アクティブなフォルダが画面外にあるときも、開いた直後に同じく表示されること。
4. **キーボードで辿る**: 表示域の最終行から Down を押し続けると、毎回カーソル行が描かれて表示域に入ること。
   Up で先頭まで戻れること。論理行の順序が仮想化の前後で変わらないこと (`visible_rows()` との突き合わせ)。
5. **折りたたみ前後の位置**: 中ほどまでスクロールした状態で、表示域より上の枝を畳む・開くと、先頭の path と
   行内の位置が保たれること (既存の `actual_folder_pane_deep_scroll_anchor_...` を大きな木で)。
6. **ヒット情報**: スクロール後の可視行への単クリック・ダブルクリック・矢印・右クリック (貼り付けメニューの path)
   が、その行の path で結果を出すこと。D&D の行一覧 (`main_drop_geometry`) が可視行と一致すること。
7. **データ側の同値性**: 隠しフォルダが `membership` にだけある場合に、`reveal_active` が今と同じ所で止まること。
   大文字小文字・区切り文字の違う現在地・カーソルでも、`is_active` / `is_cursor` が今と同じになること。
8. **行の高さ** (P1-1): 24pt を超える行の実高、loading / error 行を含めて全行の割り当てが `row_height` と一致すること。
   pitch の増減 (`pixels_per_point` やフォントの変更) の前後で、描画範囲が表示域を覆い、現在地の reveal が
   新しい座標で中央に来ること。末尾の半端なページ、表示高ゼロ・1 行未満でも panic せず描くこと。
   paths と pitch が同時に変わる pass の restore / reveal。
9. **同じ pass で範囲外へ出た行への AccessKit 要求** (P1-2): 前 pass で見えていた行 B への `Click` と、その上の
   枝 A を開くポインタ入力 (子を大量に挿入して B を描画範囲の外へ押し出す) を同じ pass に入れても、B が起動
   されること。B が描かれる場合に二重に起動しないこと。`ScrollIntoView` も同じ形で効くこと。
10. **自動スクロールと手の入力の取り合い**: `FollowingRefresh` 中に、ツリーの内側 / 外側でのホイール、慣性の
    停止 (今の `Idle` への戻し条件) が今と同じになること。ゲームパッドの上下で可視の境界を越えると reveal すること
    (`handle_tree_key` 共通)。
11. **貼り付けメニュー**: メニューを開いた元の行がスクロールで描かれなくなっても、メニューが同じ path と位置で
    残り、貼り付け先がその path になること。
12. テストは本番の focus policy (`egui_focus_policy`) を入れた Context で行う (P3-8)。
13. 既存の §1.828 の入力の関門・ツリーのテスト一式 (`folder_pane`、`ui_folder_pane`、`folder_pane_refresh_tests` 系、
   `ui_folder_pane_drop_tests`、`main_chrome` の r6 系) が通ること。画面外の行の矩形や AccessKit ノードを直接見て
   いるテストは、可視の行を見る形に直す (検査の意味は変えない)。

性能の比較 (同じ合成ツリー・同じ手順で仮想化の前後):
headless (`perf827` の計測、master に入れない) の行数・galley・時間と、portable-smoke の
`folder_pane_ms`・`App::update`・`platform_ms`・accesskit ノード・初回の最大停止・スクロール中の fps と UI スレッド。

## 11. 計装 (`frame/root_outside`)

update の外の区間を測るために、perf ログが有効なときだけ動く型付きの計装を足した。
`egui_wgpu::frame_diag` が ROOT の pass と GPU の各段の時間・数を持ち、App が次のフレームで perf イベントに出す。
記録する区間は、egui の pass、platform output (accesskit の受け渡し)、tessellate、GPU の textures / buffers / acquire /
encode / submit / release / present、イベント待ち。数は accesskit のノード、shape、mesh、頂点、index、texture delta。
perf ログが無効なら記録しない。

区間の意味 (Sol 設計レビュー P2-4・P2-5・P3-6 で直した):
- `run_ui_ms` は `integration.update` 全体 (この operation のすべての egui pass、retry、App の logic と UI を含む)。
  App の時間を引いた「egui だけ」の値は出さない (pass の数と App の区間が対応しないため)。
- `paint_ms` は tessellate の後から post_rendering まで。GPU の各段 (`gpu_*`) はその内訳で、足し合わせる
  独立の項目ではない。どれも CPU 側の経過時間。
- `between_root_ops_ms` は前の ROOT operation の終わりから今の operation の入口まで。イベント待ちのほかに、
  ほかの viewport の処理や logic だけの pass を含む。
- painter は、eframe が始めた ROOT operation の中でだけ記録する (`begin_root_pass` 〜 `finish_root_pass`)。
  別のフレームの記録を取り込まない。paint しなかった operation は `gpu_painted=false`。
- perf ログが無効なら、有効フラグを読むだけで時計を読まない。

**提案**: この計装は製品に残す。利用者の perf ログにも、update の外の内訳が出るようになる。
計測専用の切り替え (`MIV_TREE827_NO_ACCESSKIT`) と headless の計測テストは master に入れない。

