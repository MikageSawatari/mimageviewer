# 一覧の表示順で前後の本へ移る操作 (§1.444)

2026-10-26 第 1 版 (ClaudeCode)。設計・作業指示・検収 = ClaudeCode、実装 = Codex、独立レビューは通常どおり。
v4.9.0 の候補。コードの事実は 2026-10-26 時点の master (`a52b8eaae`) を読んだもので、実行時の観測は無い。

## 1. 目的と前提

- 報告 (5ch >>613・>>617、用途は報告者に確認済み): スマートフィルタで絞り込み、日付の逆順に並べたサムネイル一覧で、
  **表示されている本 (ZIP / フォルダ等) を、表示されている順に次々と開いて確認したい**。
- 既存の操作はこの用途に合わない (コード確認):
  - `FsCtrlNavPrev/Next` (Ctrl+↑↓) は `handle_fullscreen_ctrl_nav_context` (`src/ui_fullscreen.rs` 約 42078) から
    `start_folder_nav` (`src/app.rs` 約 57675) の **フォルダの木の DFS** へ進む。順序は `FolderTreeOptions::from_settings` の
    `folder_tree_sort_order` で、一覧の並べ替え・絞り込みとは別。
  - `FsSiblingPrev/Next` (Ctrl+PageUp/Down) は同じ親の兄弟をたどる別操作。
  - Ctrl+F の絞り込み中は、どちらも何もしない (`LocalFilterActive`、約 42236)。
- 本を開くと、一覧の `items` / `visible_indices` / `details_order` は本のページで置き換わり、**親の一覧の順序はどこにも残らない**
  (通常フォルダの `save_leaving_folder_grid_position` は scroll と選択だけを保存する)。
- 近い前例は ★固定 (`activate_snapshot`、`src/app/snapshot_ops.rs` 約 912): 開いた時点の一覧を **凍結した列** として持ち、
  `snapshot_navigate` で前後へ移る。ただし `visible_indices` だけを取り、詳細表示の並べ替え (`details_order`) は取らない。
  スマートフォルダの表示順ナビゲーションは一時的な UI の絞り込みを **意図して無視する** ので、今回の用途とは逆。

## 2. 決めること (案)

### 2.1 新しい操作

- `KeyAction::FsListOrderPrev` / `FsListOrderNext` (仮名) を追加する。説明「一覧の表示順で前の本を開く / 次の本を開く」。
- 文脈は `KeyContext::FsCommon` (フルスクリーン) だけ (v1)。一覧 (Grid) 側の同等操作は足さない (§4)。
- **既定キーの案: Ctrl+Shift+↑ / Ctrl+Shift+↓** (2026-10-26 時点で既定の割り当てが無い)。決定は利用者に確認する。
- `press_multiplicity` は既存の Ctrl+↑↓ と同じ `EachPhysicalPress`。ゲームパッド・リング操作には v1 では足さない (§4)。
- 既存の Ctrl+↑↓ (DFS) と Ctrl+PageUp/Down (兄弟) の挙動は変えない。

### 2.2 順序をいつ・何を取るか (凍結した列)

- **取る時点**: main の viewer context で、**通常フォルダの一覧 (`TopLevelGridSurface::Folder`) から本を開いたとき** (グリッドからの
  open で `grid_physical_navigation` に入る直前。Enter・ダブルクリック・明示の「ページ表示で開く / ページ一覧で開く」を含む)。
- **取るもの**: `current_grid_order()` (詳細表示なら `details_order`、それ以外は `visible_indices`) の順で、本として開ける行
  (Folder / Zip / Pdf / Epub / ConvertibleArchive など、既存のコンテナ分類に従う) だけを `Arc<[ListOrderEntry { path, kind }]>` に写す。
  絞り込み (スマートフィルタ・ファセット・Ctrl+F・評価フィルタ) と並べ替えは、`current_grid_order()` が既に反映している。
  画像・動画などコンテナでない行は列に入れない。
- **現在位置**: 開いた行の path を anchor として持つ。
- **凍結**: 取ったあとの一覧の変更 (絞り込み・並べ替えの変更、ファイルの追加・削除) は列に反映しない。
  一覧へ戻って本を開き直したときに、新しい列を取り直す。live に追従させる仕組みは作らない (状態の組み合わせを減らす。§5)。
- **持ち主**: viewer context の所有する状態 (`ViewerContextBundle` に載る型付きの 1 つ) にする。App 全体の bool や Option を増やさない。
  別のフォルダへ移る・一覧へ戻って別の本を開く・別の surface (評価・スマートフォルダ・検索・コレクション) に入る と破棄する。

### 2.3 移るとき

- 現在の本 (anchor) の、列の中の前 / 次の要素を開く。開き方は、**今の本を開いたときと同じ**
  (直接ページ表示か、ページ一覧か。`auto_open_for_current_container` の規則に合わせる) にし、フルスクリーンを保つ。
- 開く処理は、Ctrl+↑↓ が目的地を決めた後に使う既存の経路 (`apply_folder_nav_result` 相当: 閉じずに読み込み → 再オープン、
  ZIP / PDF の列挙待ち、`book_nav_resume` に従う開始ページ) を使い、目的地の探索 (DFS worker) だけを列からの選択に置き換える。
  新しい開き方を作らない。
- **端**: 先頭の前・末尾の次は、既存の `FsBoundaryHint` と同じ形で「一覧の先頭 / 末尾です」と知らせて止まる (ループしない)。
- **列の要素が開けない** (削除された、読めない): 既存の開けないときの通知を出して、その場に留まる。次の要素へ自動で飛ばない
  (まれな失敗は既定で割り切る)。
- 連打: 既存の `MAX_PENDING_NAV` と同じ考えで、読み込み中の追加の押下は 1 件にまとめるか、既存の nav lock に従う (実装で既存経路に揃える)。
- 一覧へ戻る (Esc / BS): 最後に開いていた本の行を選んで一覧へ戻る (既存の `save_leaving_folder_grid_position` を、移動のたびに
  現在の本で上書きする)。

### 2.4 使えない場面

- 列を持っていないとき (通常フォルダの一覧以外から開いた本、DFS で入った本、起動時の直接 open など) は、
  「一覧から開いた本でだけ使えます」と短く知らせて何もしない。DFS へ黙って切り替えない (操作の意味を混ぜない)。
- detached viewer (F12 の独立窓・複数ウィンドウ) では v1 では使わない (同じ通知)。§4 参照。

## 3. 状態と所有の不変条件

- 列は main の viewer context が所有し、別の context (detached) へ渡さない・読ませない。
- 列は凍結され、取ったあと main の一覧や設定の変更で書き換わらない。
- 列を使うのは `FsListOrderPrev/Next` だけ。既存の Ctrl+↑↓・兄弟移動・★固定・スマートフォルダ・コレクションの経路は列を読まない。
- 破棄の契機は 2.2 の「持ち主」に列挙したものだけ。read-only の open / close で別 context の状態を変えない。

## 4. 範囲外 (v1) と理由

- **一覧側 (Grid) の同等操作**: 一覧に居るときは見えている順に選んで開けばよいので、v1 では足さない。
- **detached / 複数ウィンドウ**: 独立窓は main の後からの絞り込み・場所変更を取り込まない境界がある
  (`detached_independent_session_blocks_folder_nav`)。列を bundle に載せて渡す設計は可能だが、F12 往復・promote・park の
  組み合わせが増えるので、v1 では main のフルスクリーンに限る。要望があれば次の段で検討する。
- **評価一覧・スマートフォルダ・コレクション・検索結果**: それぞれ既存の表示順ナビゲーションや検索スコープがあるので、v1 では新しい操作の対象にしない。
- **ゲームパッド・リング**: v1 では足さない (キーボード操作の要望のため)。

## 5. 検討した単純化 (CLAUDE.md「設計の簡素化」)

- **凍結した列にする**: 開いた後の一覧の変更に追従させると、絞り込み変更・削除・並べ替え・再読込と本の読み込み中の組み合わせを
  すべて扱う必要がある。凍結にすれば、取る時点と破棄の時点だけを正しく扱えばよい。報告者の用途 (絞り込んで順に見ていく) は凍結で満たせる。
- **main のフルスクリーンに限る**: detached・複数ウィンドウの所有境界との組み合わせを v1 から外す。
- **既存の開く経路を使う**: 目的地の決め方だけを差し替え、開く・列挙を待つ・再オープン・戻るは既存の経路に乗せる。
- **端で止まる・開けないと止まる**: ループや自動スキップの分岐を作らない。

## 6. テスト計画

- 列の取得: スマートフィルタ・ファセット・Ctrl+F・評価フィルタ・詳細表示の並べ替え (昇降順) のそれぞれで、列が画面の順・対象と一致する。
  画像・動画の行は入らない。名前・日付の昇降順。
- 移動: 前後・先頭 / 末尾での通知・連打・直接ページ表示とページ一覧の両方・`book_nav_resume` の有無。開けない要素で留まる。
- 戻る: 最後に開いた本の行が選ばれる。
- 使えない場面: 列が無いとき・detached・評価一覧などで通知だけ出して何もしない。既存の Ctrl+↑↓ / 兄弟移動が変わらない。
- 破棄: 別フォルダへ移る・別 surface に入る・一覧から別の本を開くで列が入れ替わる / 消える。別 context が影響を受けない。
- keymap: `ini_name` / `context` / `trigger` / `default_chords` / `ALL_ACTIONS`、`docs/keymap.ini.default` の再生成、`docs/keymap-spec.md`。
- 実アプリ: 隔離 data-dir で、スマートフィルタ + 日付降順の本の一覧から順に開く (ClaudeCode が PC 使用の了承を得て行う)。

## 7. 文書

- `docs/keymap-spec.md` のフルスクリーンの表、`docs/fullscreen-navigation-consistency.md` §2・§3、
  マニュアル (`fullscreen.html`・`shortcuts.html`) に新しい操作と「一覧から開いた本でだけ使える」ことを書く。
