# 一覧の表示順で前後の本へ移る操作 (§1.444)

2026-10-26 第 3 版 (ClaudeCode)。第 3 版の独立レビューは ACCEPT WITH CHANGES で、指摘 4 件を反映済み (§9)。設計・作業指示・検収 = ClaudeCode、実装 = Codex、独立レビューは通常どおり。
v4.9.0 の候補。コードの事実は 2026-10-26 時点の master を読んだもので、実行時の観測は無い。

- 2026-10-26 利用者決定: **Ctrl+↑↓ (フォルダの木の順) は変えず、別操作にする**。一覧の順は「その一覧の中」しか
  たどれず、サブフォルダへ降りられないため、既定を置き換えると今の使い方ができなくなる。
- 第 1 版・第 2 版は独立レビュー (Codex gpt-6.1-sol xhigh) で REJECT。第 2 版の残件は、開く要求の由来を
  採用の時点まで運ぶ配管 (Direct に載せる場所が無い、warm PDF は先に採用される、ZIP/PDF の移動は後で採用される、
  ★固定は採用口を通らない) と、表示先・gate・停止条件の記述。第 3 版は **今の本の位置を状態として持たず、
  表示中の本から毎回求める** 形に改め、由来の配管と保持・破棄の遷移表そのものを無くした (§5・§9)。

## 1. 目的と前提

- 報告 (5ch >>613・>>617、用途は報告者に確認済み): スマートフィルタで絞り込み、日付の逆順に並べたサムネイル一覧で、
  **表示されている本 (ZIP / フォルダ等) を、表示されている順に次々と開いて確認したい**。
- 既存の操作はこの用途に合わない (コード確認):
  - `FsCtrlNavPrev/Next` (Ctrl+↑↓) は `handle_fullscreen_ctrl_nav_context` (`src/ui_fullscreen.rs` 約 42078) から
    `start_folder_nav` (`src/app.rs` 約 57675) の **フォルダの木の DFS** へ進む。順序は `folder_tree_sort_order` で、
    一覧の並べ替え・絞り込みとは別。
  - `FsSiblingPrev/Next` (Ctrl+PageUp/Down) は同じ親の兄弟をたどる別操作。
- 本を開くと、一覧の `items` / `visible_indices` / `details_order` は本のページで置き換わり、親の一覧の順序は残らない。
- 土台にする前例: `FolderNavMode::SmartFolder { state, fullscreen }` (`src/app.rs` 約 4491) は入力時点の snapshot を mode に持たせ、
  worker が UI state を読まずに行き先を決める。行き先が決まった後の事前走査・`apply_folder_nav_result`・採用は DFS と共通。

## 2. 操作

- `KeyAction::FsListOrderPrev` / `FsListOrderNext` を追加する。説明「一覧の表示順で前の本を開く / 次の本を開く」。
- 文脈は `KeyContext::FsCommon` (フルスクリーン) だけ。一覧 (Grid) 側の同等操作は足さない。
- **既定キー: Ctrl+Shift+↑ / Ctrl+Shift+↓**。既定の割り当てとの衝突なし (レビューで確認。修飾キーは完全一致なので
  Ctrl+↑↓・Shift+↑↓ の音量操作とも区別される: `src/keymap.rs` 約 1190・1238)。
- `press_multiplicity` は Ctrl+↑↓ と同じ `EachPhysicalPress`。新しいゲームパッド割り当ては足さない。
- 既存の Ctrl+↑↓・Ctrl+PageUp/Down の挙動は変えない。

## 3. 状態: 最後に通常フォルダ一覧から本を開く操作を受け付けた時点の、凍結した列

### 3.1 型と持ち主

```rust
struct ListOrderSnapshot {
    owner: ViewerContextId,               // 取得した時点の main context
    entries: Arc<[ListOrderEntry]>,       // 一覧の表示順。凍結。path は重複しない
}
struct ListOrderEntry { path: PathBuf, kind: ListOrderEntryKind } // Folder / Zip / Pdf / ConvertibleArchive
```

- `Option<ListOrderSnapshot>` を **`ViewerContextBundle` の 1 field** にする。context が持つ状態なので、park / mount の
  bundle swap で context と一緒に動く。field の初期化・bundle swap・fork 時の `keep_in_main` の分類は明示する
  (fork の子は空。`src/app/viewer_context_registry.rs` 約 3199・3888 の既存の分類に並べる)。registry の table の遷移規則は変えない。
- **今の本の位置 (index) は持たない**。使うたびに、表示中の本のパスが `entries` のどこにあるかを求める (§3.4)。

### 3.2 取得するもの

- `current_grid_order()` (`src/app.rs` 約 72808。詳細表示なら `details_order`、それ以外は `visible_indices`) の順で、
  `GridItem::Folder` / `ZipFile` / `PdfFile` (EPUB を含む) / `ConvertibleArchive` の行だけを写す。画像・動画などの行は入れない。
  絞り込み (スマートフィルタ・ファセット・Ctrl+F・評価フィルタ) と並べ替えは `current_grid_order()` が既に反映している。
- 取った後の一覧の変更は反映しない (凍結)。

### 3.3 取得する時点 (置き換えるだけ)

- **main の通常フォルダの一覧から、本の行を開く操作を受け付けた時点**で、その context の列を新しい列に置き換える。
  開く要求には何も載せない。要求の採用・取消・失敗を待たない。
- 取得は 1 つの helper `capture_main_folder_list_order(idx)` に集め、本の行を開く要求を作る 3 か所から、**load を始める前に**呼ぶ:
  - 通常フォルダ・ZIP / PDF (`grid_physical_navigation` `src/app/collection_grid.rs` 約 1355 を呼ぶ入口)
  - 変換対象の書庫 (`MainGridArchive`。`grid_physical_navigation` を通らない: `src/app.rs` 約 57428、`src/ui_main.rs` 約 16782、
    `src/app.rs` 約 59301)
- Enter・ダブルクリック・明示の「ページ表示で開く / ページ一覧で開く」・既存のゲームパッドの決定は、上の入口を通るので同じ規則になる。
  実装時に入口を列挙して確かめる。
- helper は次の条件をすべて満たすときだけ取る。満たさなければ何もしない (既存の列も消さない):
  main context である / 一覧の surface が `TopLevelGridSurface::Folder` で ★固定が無効 (`is_snapshot_active()` が偽) /
  要求の source が `Collection` / `Rating` owner でない / スマートフォルダの入口 (`begin_smart_grid_container_navigation`) に入らない。
  評価一覧・スマートフォルダ・検索結果・閲覧履歴・コレクション・ブックマーク・サブフォルダ展開・ドライブ一覧からは取らない。
- 下位の汎用 load 関数では取らない (DFS・起動時 open・アドレスバー入力が混ざるため)。
- 開く操作が取消・失敗しても、列は「いま見ている一覧」の写しなので害は無い。元の列へ戻す処理は作らない。次に本を開けば、また置き換わる。

### 3.4 使える条件と今の位置 (毎回求める)

述語 `list_order_position() -> Option<(Arc<ListOrderSnapshot>, usize)>` を 1 つだけ作り、操作の受付で毎回呼ぶ。次をすべて満たすとき位置を返す:

1. 列がある
2. `snapshot.owner` が registry の現在の main id と一致し、mount 中の context がその main である
   (promote で main id は新しく発行され、旧 id が main に戻ることは無いので、旧 main に残った列は二度と使えない)
3. 表示先が `ViewerPresentation::MainWindow` か `Fullscreen` (通常の専用フルスクリーン。App の初期値) で、
   `DetachedWindow` でも独立 owner でもない
4. 一覧の surface が `TopLevelGridSurface::Folder` で、★固定が無効
5. 表示中の本 (`effective_folder()`) のパスから位置を求められる。求め方は 2 段だけ:
   1. `entries` のどれかの `path` と完全に一致すれば、その位置 (比較は `folder_tree::path_eq`)。
   2. 一致しなければ、**`Folder` の行だけ**を対象に、表示中のパスをその行の配下に含む行の位置 (パス成分の境界を守る既存の
      `path_in_subtree_ci` `src/app/metadata_ops.rs` 約 243 と同じ比較)。Folder 行から中の書庫へ着地した場合
      (行 `P/F` の中身が音声と `book.zip` だけ → 停止条件は音声で真、再オープンは `P/F/book.zip` を開く: `src/folder_tree.rs` 約 587、
      `src/app.rs` 約 60168・60203) に、行 `P/F` の位置になる。
   - 比較する値は `effective_folder()` でよい (レビュー確認: EPUB のキャッシュ変換は lease が元の論理パスを持ち、
     変換書庫は `archive_source_override` が元のパスを返す。`src/pdf_loader.rs` 約 376、`src/app.rs` 約 23733・36874)。
   - EPUB の「PDF として保存」の後は、保存先の PDF を開く別のパスへの移動として扱う (`src/ui_dialogs/epub_convert.rs` 約 664、
     `src/app.rs` 約 39585)。保存先が列に無ければ位置は求まらず、通知になる。元の EPUB への別名や位置の保持は足さない。

満たさないときは「一覧から開いた本で使えます」と短く知らせて何もしない。DFS へ黙って切り替えない。

- 5 の帰結: 一覧から開いた本・この操作で移った本はもちろん、DFS・履歴・評価一覧から開いた物理の本などで**同じ列に含まれる本**へ
  来たときも使える (評価一覧から開いた物理の本も surface は `Folder` になる: `src/app.rs` 約 36810)。
  このとき使うのは、**最後に通常フォルダ一覧から本を開く操作を受け付けた時点の順**で、いま見ている評価一覧などの順ではない。
  そこに含まれる本なら前後が決まる。列に無い場所へ移ると自然に使えなくなり、
  破棄の処理は要らない。★固定に入ると 4 で使えなくなり、解除すると戻る。
- 列の位置を index として持たないので、移動の採用・取消・ZIP / PDF の後からの採用・hydration・内側の書庫の open のどれに対しても、
  受け渡しや更新の処理が無い。

## 4. 移動

### 4.1 行き先の決め方: 新しい `FolderNavMode`

- `FolderNavMode::ListOrder { snapshot: Arc<ListOrderSnapshot>, from_index: usize }` を足す (フルスクリーン専用)。
  `from_index` は押した時点の §3.4 の位置。`start_folder_nav` → `spawn_folder_nav` の既存経路に乗せ、
  worker の中で行き先の決め方だけを分岐する (SmartFolder と同じ形)。
- worker: `from_index` から前 / 次へ 1 行ずつ見て、**Ctrl+↑↓ (フルスクリーン) が候補に使う停止条件**
  (`folder_should_stop_with_memo`、同じ `FolderTreeOptions` と `archive_policy`) を満たす最初の行を行き先にする。cancel token を各行で確かめる。
  列は有限なので必ず終わる。
- 停止条件が実際に判定する内容 (`src/folder_tree.rs` 約 310・532・567。仕様とテストはこれに合わせる):
  - 通常フォルダ: 直下に表示対象の画像・動画・音声ファイルがあれば真。**ZIP / PDF だけを含むフォルダは偽** (飛ばす)。
  - ZIP: 表示できる画像の entry があれば真。PDF / EPUB: 中身を読まずに真。変換対象の書庫: `archive_policy` と準備の解決による。
  - 読めることの完全な保証ではない。停止条件を通った後の open の失敗は、既存のログ + 通知で扱う (まれな失敗は割り切る)。
  - 音声だけのフォルダは真になるが、着地時の再オープン対象に音声は無い (`src/app.rs` 約 60168)。これは Ctrl+↑↓ と同じ既存の挙動で、
    ここでは変えない。
- 行き先が決まった後の事前走査は DFS と同じコード (`spawn_folder_nav` の `scanned` 部分) を通す。UI スレッドに同期走査を戻さない。
- 第 1 版の「開けない行で止まる・自動で飛ばさない」は撤回した。空のフォルダが列の途中にあると先へ進めなくなるため。
  飛ばす基準は Ctrl+↑↓ と同じで、新しい基準は作らない。

### 4.2 受付の gate と連打

- 入力は `FsCtrlNavPrev/Next` と同じ gate (modal・入力欄・`viewer_mutation_modal_active`) を通す。
- **移動の準備中は、新しい操作を受け付けない**。既存の Ctrl+↑↓ は開始時に `FolderItems` の sequence を作り、入力 handler は
  `start_folder_nav` の mode 比較より先に、この sequence / 表示待ち lock で新しい行き先を拒否する
  (`src/ui_fullscreen.rs` 約 13336・42091、`src/app.rs` 約 12375)。新しい操作もこの既存の gate に従う。
  拒否する期間は `folder_nav_pending` だけでなく、後続の準備・hydration・表示確定待ちまでを含む (既存の gate の範囲のまま)。
- **新しい操作の handler は、sequence / 表示待ち lock 中なら明示的に return する**。既存の Ctrl+↑↓ の lock 分岐は、拒否だけでなく
  進行中の mode を取って継続を受け付ける処理 (`inflight_fullscreen_folder_nav_mode`、`src/ui_fullscreen.rs` 約 42102) を含むので、
  新しい操作はこの継続受付に入らない。`inflight_fullscreen_folder_nav_mode` は `ListOrder` を継続対象として返さない。
- `folder_nav_mode_same_kind` を偽にするだけでは拒否にならない (偽なら `start_folder_nav` は旧 worker を cancel して新しく始める:
  `src/app.rs` 約 57696)。拒否は上の handler の return で行う。共通入口に到達した場合も、進行中の `ListOrder` を cancel・再開始しないことを検証する。
- この操作の準備中に Ctrl+↑↓ を押した場合、Ctrl+↑↓ の準備中にこの操作を押した場合も、既存の gate で拒否される。cancel して乗り換える処理は作らない。
- 累積 (`MAX_PENDING_NAV`) には乗せない。新しい状態 field は足さない。

### 4.3 着地

- 採用・再オープン・ZIP / PDF の列挙待ち・hydration 後の再オープン・開始ページ (`book_nav_resume`) は、
  **同じ行き先へ Ctrl+↑↓ (`FolderNavMode::Fullscreen`) で着いたときと同じ**にする。`apply_folder_nav_result` の
  `Fullscreen` の後処理と、ZIP / PDF が `PhysicalHistoryTransition` を経て後から採用される経路を共有し、新しい開き方を作らない。
  `ListOrder` が `Fullscreen` と同じに扱われる必要のある分岐 (`StartupListIntent::folder_navigation`・`history_trigger`・`perf_tag` など) は、
  実装時に `FolderNavMode` の match を全部たどって揃える。ただし `inflight_fullscreen_folder_nav_mode` の継続許可は除く (§4.2)。
- 着いた本の位置は §3.4 で求め直すので、着地後に列へ書き込む処理は無い。
- 端 (先頭の前・末尾の次、または残りの行がすべて停止条件を満たさない): 既存の `FsBoundaryHint::Edge` の形で
  「一覧の先頭です / 一覧の末尾です」と知らせて止まる。ループしない。
- フルスクリーンは保つ。閉じる・戻る (Esc / BS) は**既存の規則と設定のまま** (Backspace はページ一覧へ、Esc は条件により親一覧へ)。
  親へ戻ったときの選択は、既存の `effective_folder()` の名前による選択要求に任せる。
- 採用前の失敗 (worker の cancel・走査失敗) は既存の準備経路で止まる。採用後のまれな失敗は既存のログ + 通知で割り切り、
  専用の巻き戻しは作らない。

## 5. 状態遷移

列を書き換えるのは §3.3 の取得だけで、出来事ごとの保持・破棄の分岐は無い。使えるかどうかは §3.4 が毎回決める。

| 出来事 | 列 | 操作が使えるか (§3.4) |
| --- | --- | --- |
| main の通常フォルダの一覧から本を開く操作を受け付けた | 新しい列に置き換え | 開いた本が表示されれば使える |
| その open が取消・失敗した | 置き換えたまま (いま見ている一覧の写し) | 一覧に居るので操作の文脈 (FsCommon) に無い |
| この操作で移動した (ZIP / PDF の後からの採用・hydration を含む) | 変えない | 着いた本が列にあるので使える。Folder 行から中の書庫に着いたときは §3.4 の 5-2 で行の位置になる |
| EPUB を「PDF として保存」して保存先を開いた | 変えない | 保存先が列に無ければ通知 |
| DFS・兄弟移動・履歴・アドレスバー・ブックマーク・評価一覧から開いた物理の本 | 変えない | 着いた本が列にあれば使える (評価一覧の順ではなく保持した列の順)。無ければ通知 |
| 親一覧へ戻った・他の surface へ移った | 変えない | 一覧に居る間は文脈に無い。次に本を開けば置き換わる |
| ★固定を有効にした / 解除した | 変えない | 有効の間は 4 で使えない。解除で戻る |
| F12 で同じ context を別窓へ出す / 戻す (linked) | 変えない | 別窓の間は 3 で使えない |
| context の park / mount | bundle と一緒に動く | mount 中の main なら使える |
| fork | 子は空 (`keep_in_main` で明示) | 子では使えない |
| promote | 新しい main は空。旧 main の payload に残った列は 2 で使えない | — |

## 6. 範囲外 (v1) と理由

- **一覧側 (Grid) の同等操作**: 一覧に居るときは見えている順に選んで開けばよい。
- **detached・複数ウィンドウ・F12 linked の別窓での使用**: §3.4 で通知だけにする。列を別 context に渡す設計は、
  F12 往復・promote・park の組み合わせが増えるので v1 では作らない。
- **評価一覧・スマートフォルダ・コレクション・検索結果・閲覧履歴・サブフォルダ展開からの取得**: それぞれ既存の表示順ナビゲーションや検索の範囲がある。
- **新しいゲームパッド・リング操作**: キーボード操作の要望のため。

## 7. 検討した単純化 (CLAUDE.md「設計の簡素化」)

- **凍結した列**: 一覧の変更に追従させると、絞り込み変更・削除・並べ替え・再読込と移動中の組み合わせをすべて扱う必要がある。採用。
- **既定の Ctrl+↑↓ を置き換える案**: サブフォルダへ降りられなくなる (今の使い方ができなくなる) ので不採用 (利用者決定)。
- **今の位置を持たず、表示中の本から毎回求める** (第 3 版): 第 2 版は位置 (index) を状態に持ったため、開く要求・移動要求の由来を
  採用の時点まで運び、出来事ごとに保持・交換・破棄を決める配管が要った (Direct・warm / cold PDF・変換書庫・ZIP の後からの採用・
  ★固定・内側の書庫ごと)。位置を「データ (列と表示中の本) から導く」形にすると、列を書き換える時点は取得の 1 か所だけになり、
  取消・置換・後からの採用の組み合わせを扱う必要が無くなる。採用。
  - 引き換え: DFS・履歴・評価一覧経由で列に含まれる本へ来たときも使える。列は「最後に通常フォルダ一覧から本を開く操作を受け付けた時点の順」
    なので前後は決まり、害は無いと判断した (レビューも許容)。説明文書でこの定義を統一する。
  - 第 1 版の「パスの変化で保持・破棄を決める」とは違う。第 3 版は保持・破棄をしない。パスは位置を引くためだけに使う。
- **既存の `FolderNavMode` に 1 variant 足す**: 走査・事前走査・採用・再オープン・ZIP / PDF の待ちを DFS と共有できる。採用。
- **準備中は受け付けない**: 既存の gate がそうしている。乗り換えや累積を作らない。採用。
- **飛ばす基準を Ctrl+↑↓ と同じにする**: 新しい「本として開けるか」の判定を作らない。採用。
- **戻り方を既存のまま**: 一覧の位置を覚え直す専用処理を作らない。採用。
- **F12 linked・detached では使わない**: owner id と表示先の確認だけで済み、registry の遷移規則に手を入れない。採用。

## 8. 変更範囲・テスト・文書

### 8.1 変更範囲

- `src/keymap.rs`: `KeyAction` の 2 variant、`ini_name()` / `description` / `context()` / `trigger()` / `press_multiplicity` /
  `default_chords()` / `ALL_ACTIONS` などの各表。egui 側の押下数処理。
- native 動画: whitelist は `FsCommon` を拾うが、App 側の resolver / dispatcher は既存 4 操作を明示列挙している
  (`src/app/native_video.rs` 約 12571・12978)。2 操作を足す。動画ショートカットのヘルプにも足す。
- `src/ui_fullscreen.rs`: `FsCommon` の dispatch。gate は `FsCtrlNavPrev/Next` と同じ。
- `src/app.rs`: `FolderNavMode::ListOrder`、worker の分岐、`FolderNavMode` の全 match、§3.3 の helper と 3 か所の呼び出し、§3.4 の述語。
- `src/app/viewer_context_registry.rs`: `ViewerContextBundle` に `list_order: Option<Arc<ListOrderSnapshot>>`。初期化・swap・
  fork の `keep_in_main` 分類。名前が `pending` で終わらないので `context_async_owner_registry_audits_bundle_pending_fields` の登録対象ではない。
- `docs/keymap.ini.default` は生成物として再生成する。

### 8.2 detached 凍結規則 (CLAUDE.md・`docs/detached-rework-plan.md` §2)

`ViewerContextBundle` への field 追加と、表示先の判定は detached の述語 / context 経路に触れる。
- 第 2 版の独立レビューで、**症状パッチではなく構造的な修正である**ことに合意を得た (2026-10-26)。条件は、
  「registry の遷移を変えない」を table の遷移規則に限ること (field の初期化・swap・fork の分類は行う)、§11 への記録、表示先条件の修正。
  第 3 版はこの条件に合わせた (§3.1・§3.4 の 3)。
- 第 3 版の再レビューでも合意が維持された (2026-10-26)。触れる範囲と理由を `docs/detached-rework-plan.md` §11 に記録した。
  実装で範囲が変わったら §11 も直す。

### 8.3 テスト (handler-level と状態遷移)

- 取得: スマートフィルタ・ファセット・Ctrl+F・評価フィルタ・詳細表示の並べ替え (昇降順)・名前 / 日付の昇降順で、列が画面の順・対象と一致する。
  画像・動画の行は入らない。§3.3 の 3 か所 (通常フォルダ・ZIP / PDF・変換対象) で、load の開始前に取れる。
  評価一覧・スマートフォルダ・検索・閲覧履歴・コレクション・サブフォルダ展開・★固定・Collection / Rating owner・main 以外では取らず、既存の列も変えない。
- 位置 (§3.4): 一覧から開いた本・この操作で移った本 (通常フォルダ・ZIP・cold PDF・warm PDF・EPUB のキャッシュ変換・変換対象)・
  DFS で列内の本へ来たとき・取消後に同じ列の本へ来たとき・評価一覧から開いた物理の本が列内のときに位置が出る。
  Folder 行 (音声 + 中の ZIP) から中の書庫に着地し、そこから次の行へ進める。境界違い (`book-a` と `book-a-extra`) を取り違えない。
  EPUB を「PDF として保存」した後、保存先が列に無ければ出ない。
  列外の本・★固定中・`DetachedWindow`・fork の子・promote 後の旧 context では出ない。`MainWindow` と `Fullscreen` の両方で出る。
  別 context の bundle が変わらない。
- 移動: 前後・端の通知・停止条件で飛ばす行 (ZIP / PDF だけのフォルダ・空のフォルダ・消えた行)・残りが全部飛ばす行のとき端の通知・
  直接ページ表示とページ一覧の両方・`book_nav_resume` の有無。
- gate: 準備中 (sequence・hydration・表示待ちを含む) の押下を受け付けず、継続受付にも入らない。共通入口に到達しても進行中の `ListOrder` を cancel・再開始しない。Ctrl+↑↓ の準備中にこの操作、この操作の準備中に Ctrl+↑↓ を押しても乗り換えない。
- 入力: 既定キー・変更した割り当て・repeat・入力欄 / modal 中・使えないときの通知・既存の Ctrl+↑↓ / 兄弟移動が変わらない・
  native 動画再生中に効く。
- keymap: 生成した `.default` と `docs/keymap.ini.default` の一致テスト。
- 実アプリ: 隔離 data-dir で、スマートフィルタ + 日付降順の本の一覧から順に開く (ClaudeCode が PC 使用の了承を得て行う)。

### 8.4 文書

- `docs/keymap-spec.md` のフルスクリーンの表、`docs/fullscreen-navigation-consistency.md` §2・§3、
  マニュアル (`fullscreen.html`・`shortcuts.html`) に新しい操作、「最後に通常フォルダの一覧から本を開いたときの、その一覧の順で移る」こと、
  Ctrl+↑↓ との違い (一覧の中だけ・サブフォルダへは降りない) を書く。

## 9. レビュー指摘への対応

### 実装時の補足 (2026-10-26)

- helper・worker の実装は `app/list_order_navigation.rs` に分け、App は bundle の投影 field を持つ。
  `grid_physical_navigation` の共通入口で通常フォルダ・ZIP/PDF を取得し、変換書庫の
  Enter / double-click / 明示指定の3入口では load 前に同じ helper を呼ぶ (helper 呼出しは計4か所)。
- 既存 `FsBoundaryHint::Edge` はページ端用の「先頭です / 末尾です」と Home/End・DFS 案内だけを
  持っていたため、typed な `FsBoundaryEdgeScope::Page | ListOrder` を付けた。
  Page の挙動は維持し、ListOrder だけ指定の「一覧の先頭です / 一覧の末尾です」を egui/native で共有する。
  これは端通知の文言を区別するための局所的な拡張で、移動用の状態 field は追加しない。
- `EachPhysicalPress` は既存Ctrl操作と同じ helper を使う。egui は複数の物理押下を数え、
  repeatだけのframeは1回に集約する。準備中は専用handlerが追加受付を拒否し、累積しない。
  native動画のrepeatは既存Ctrl操作と同じくdispatcherで無視する。eguiも全面的にrepeatを
  無視するという指定ではないので、専用no-repeat APIは追加しない。

### 第 3 版 (ACCEPT WITH CHANGES) の指摘

| 指摘 | 対応 |
| --- | --- |
| P2-1 Folder 行から中の書庫へ着地すると完全一致で位置が求まらない | §3.4 の 5: 完全一致の次に、Folder 行だけを対象に境界付きの配下判定で求める |
| P2-2 `folder_nav_mode_same_kind` を偽にするだけでは拒否にならない | §4.2: handler が lock 中に明示的に return し、継続受付に入らない。§4.3 から継続許可を除外 |
| P2-3 EPUB の「PDF として保存」は別の着地 | §3.4 の 5: 別のパスへの移動として扱い、列に無ければ通知 |
| P3-4 列の定義と取消時の意味 | §3 見出し・§3.4・§7・§8.4: 「最後に通常フォルダ一覧から本を開く操作を受け付けた時点の順」に統一。評価一覧経由の例をテストに追加 |
| §8.2 合意 | 第 3 版でも維持 (レビュー明示)。`docs/detached-rework-plan.md` §11 に記録した |

### 第 2 版 (REJECT) の指摘

| 指摘 | 対応 |
| --- | --- |
| P1-1 採用口の唯一性・必須化が成り立たない、★固定が採用口を通らない | §3.4・§5・§7: 位置を状態に持たず毎回求める。採用口に手を入れない。★固定は §3.4 の 4 で除外 |
| P1-2 `MainWindow` 限定では通常のフルスクリーンで使えない | §3.4 の 3: `MainWindow` と `Fullscreen` を許し、`DetachedWindow`・独立 owner を除外。取得は表示先を問わず main の一覧で判定 (§3.3) |
| P2-3 `Install` / `Advance` の配管と `Keep` の振り分けが未確定 | §3.3: 取得は load 開始前に置き換えるだけで、要求に何も載せない。`Advance` / `Keep` は無くした |
| P2-4 mode の乗り換え規則が既存 gate と矛盾 | §4.2: 既存の sequence / 表示待ち lock に従い、準備中は受け付けない。乗り換えを作らない |
| P2-5 停止条件の実際の判定と説明が合わない | §4.1: 実際の判定を書き、ZIP / PDF だけのフォルダは飛ばす・PDF は中身を読まない・音声だけのフォルダは既存のまま、と仕様とテストを合わせた。中の書庫への着地の記述は削除 |
| §8.2 合意の条件 (遷移規則に限る・fork の分類・§11 記録・表示先) | §3.1・§3.4・§8.2 |

### 第 1 版 (REJECT) の指摘

P1-1 (由来で決める) は第 3 版で「保持・破棄をしない」形に置き換えた。P1-2 (目的地の差し替え) は §4.1 の `FolderNavMode::ListOrder`、
P2-3 (変換書庫の入口) は §3.3、P2-4 (開き方・戻り方) は §4.3、P2-5 (Folder の定義と anchor) は §4.1・§3.4、
P2-6 (main と F12 linked) は §3.4、P2-7 (native 動画) は §8.1 で対応した。

## 10. 実装・検証記録 (2026-10-26〜27)

対象は `listorder-1444` / 基点 `a32559dca` の未コミット差分。commit・製品アプリ起動は行わない。
実装者側の独立コードレビューは、入力の直dispatchが共通受付処理を迂回する指摘を修正し、
追加の必須修正指摘なし。入力 intent は一時的な `FsKeyAction` に載せ、ROOT / forwarded ROOT /
viewport から既存のナビゲーション dispatcher を通す。準備中と利用資格なしの拒否は、
評価復帰・required-open・preview の owner を退役する前に専用 handler で判定する。

### テストの範囲

- `app/list_order_tests.rs`: 種類・凍結・重複除去、通常/詳細の名前・日付昇降順、スマートフィルタの
  facet と Ctrl+F / 評価の合成、取得受付・surface / owner 除外、論理sourceの位置、境界、★固定、
  cold/warm PDF採用、EPUB cache世代lease、PDF取消後、評価由来の物理本。
- `app/list_order_worker_tests.rs`: 停止条件、空・消失・書庫/文書のみのfolderと子孫を飛ばす規則、
  前後・端・取消、事前scanと再表示、sequence / hydration後の表示lock、Ctrl/兄弟との相互拒否、
  共通入口の非再開始、eguiの既定/再割当/repeat/入力欄/modal、端・利用不可の通知。
- `app/list_order_virtual_tests.rs`: handler→worker→分類→PhysicalHistoryTransition→prepared採用→
  hydration→再openを既存Fullscreenと比較 (通常folder・ZIP・cold/warm PDF・変換cache・音声と内側ZIP)。
  ZIP/PDFの列挙待ちでは、DeferredFsReopenが本を開く設定ではなくbook_nav_resumeを使い、保存ページへ戻ることも検査する。
- registry: bundle swap・両fork子の空・兄弟不変・park/mount・promote後の旧owner無効。
  keymap / native resolver・dispatcher / 動画help / egui operation identity / 実pointer double-click /
  受理前後の評価復帰・preview・required-open ownerもhandler-levelで検査する。

### コードの前提と検証の限界

- staged preflightで全ページを採用したZIP/PDFは、既存Fullscreenでも列挙pendingが残らず先頭へ再表示する。
  `book_nav_resume`を読むのは既存の列挙待ちDeferred経路。今回は両modeの同じ採用・再表示を共有し、
  既存Fullscreen自体の再開位置規則は変更しない。
- Collection ownerは現行APIではCollection surfaceでのみ成立するため、Folder surfaceと同時の
  fixtureは作らずsurface除外で検証する。Folder surfaceでも成立するRating ownerは直接検証する。
- PDFの実デコード・EPUB変換器・実native動画presenterの操作は行わない。PDF枚数採用は既存の
  completed-enumerate / ready-preflight seam、EPUBは実cache publication・leaseとprepared採用を使う。
- `test-lib-full.ps1` と実アプリ確認はClaudeCode側。実アプリは隔離data-dirで、スマートフィルタ＋
  日付降順の通常フォルダ一覧から開き、Ctrl+Shift+↑↓、両端、ページ一覧経由、既存Ctrl+↑↓、native動画、
  ★固定・F12別窓での通知を確認する。

### 自動検証

ログは `target/list-order-verification/`。ネイティブbuildは `MSBUILDDISABLENODEREUSE=1`。
初回TurboJPEGの並列MSBuildが非ゼロになったため、診断の単一並列build後、ネイティブ依存のcold buildは
`CARGO_BUILD_JOBS=1`で実行する。構築済み依存を使うRust再コンパイル・テストは通常並列でよい。
これはビルド実行環境だけの変更で、ソース/配布設定は変更しない。

関連nextestの初回は385件中380成功・5失敗。失敗は追加テストの (a) egui repeat無視という仮定、
(b) writer公開snapshotを通さない読書位置の直接DB書込み、(c) Folderへのfile_operation_path使用。
正本と既存helper契約を再照合し、writer FIFOからの保存位置公開をassertし、Folderのパス取得も修正した。
読書位置の期待2は維持。製品コードを変えず、同じ385件の再実行は全件成功した。
取得・位置の15件は重複を避けて同nextestから除外し、`test-focused.ps1`で別に全件成功した。

関連nextestのfilter (以下 `$relatedFilter`):

```powershell
$relatedFilter = '(test(/list_order/) or test(/keymap::tests::/) or test(/folder_nav/) or test(/fullscreen_ctrl_nav/) or test(/fullscreen_sibling_nav/) or test(/counted_action/) or test(/boundary_notice_setting_only_hides_edge_hints/) or test(/static_and_native_boundary_titles_use_the_same_wording/) or test(/context_async_owner_registry_audits_bundle_pending_fields/) or test(/raw_text_edits_are_ime_aware_or_explicitly_exempt/)) and not test(/app::list_order_tests::/)'
```

| コマンド | exit code・結果 | ログ |
| --- | --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 初回1 (TurboJPEG MSBuild)、単一並列で再実行0 | `check.*.log` (再実行) |
| `cmake --build target/debug/build/turbojpeg-sys-4adae978226e4e06/out/build --target install --config Debug --parallel 1 -- /verbosity:normal` | 0、初回MSBuildの診断・依存buildのみ | tool出力 |
| `cargo check -p mimageviewer --bin mimageviewer-core --features test-script` | 0 | `check-test-script.*.log` |
| `UPDATE_KEYMAP_DEFAULT=1 cargo nextest run -p mimageviewer --lib -E 'test(/the_checked_in_default_keymap_matches_what_the_app_writes/)'` (環境変数はPowerShellで設定) | 通常並列101 (依存MSBuild)、単一並列101 (追加testのcompile error 10件)、test側修正後0・1件成功。`.default`再生成済み | `keymap-generate*.*.log` |
| `cargo nextest run -p mimageviewer --lib -E $relatedFilter` | 初回100・380/385成功、test修正後0・385/385成功。通常実行の生成一致テストも含む | `related.*.log` / `related-fixed.*.log` |
| `.\scripts\test-focused.ps1 -Path src/app/list_order_tests.rs` | 0・15/15成功 | `target/test-focused/runs/20261026T144917237Z-e99b8ea8/` |
| `cargo test -p mimageviewer --test ui_snapshot` | 0・203/203成功、snapshot更新なし | `ui-snapshot.*.log` |
| `cargo fmt --check` | 0 | tool出力 |
| `python scripts/check_ui_glyphs.py` | 0、危険グリフ0件 | tool出力 |
| `python scripts/check_site_cards.py` | 0、49 cards / 違反0件 | tool出力 |
| `git diff --check` | 0 | tool出力 |
| `cargo build --profile dev-runtime -p turbojpeg-sys` (`CARGO_BUILD_JOBS=1`) | 0、確認用profileのネイティブ依存を事前build | `dev-native-prerequisite.*.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 0、normal feature set・core/remote/EPUB worker生成、runtime DLL配置、PE依存検査runtime=4 / pe=3成功。起動なし | tool出力 / `target/vcrt-pe-reports/dev-runtime.json` |

build-devの待機を0にしたのは、先行MSBuildの終了後もidle node processが残っていたため。
実行中アプリを止めず、build script・配布構成・設定profileは変更していない。

### 確認用ビルドと手動確認

2026-10-27に `target/dev-runtime/mimageviewer-core.exe` と同protocolのRemote service・EPUB workerを生成済み。
エージェントから起動はしていない。今回の手動確認では、リポジトリルートから隔離data-dirを指定する:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe -ArgumentList '--data-dir', '.\target\dev-runtime\data'
```

通常profileで起動するコマンドは次のとおり。こちらは実利用中の `%APPDATA%\mimageviewer` を使い、
設定・キャッシュ・ログ等の実データを更新し得る。通常版とsingle-instanceの名前空間を共有するため、
インストール版・tray常駐版を終了してから起動する:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

手動で確認する操作 (未実施):

1. 通常フォルダの本一覧をスマートフィルタで絞り、日付降順にして本を開く。
   Ctrl+Shift+↓/↑でその表示順に移り、両端で一覧の端の通知が出て折り返さないこと。
2. ページを直接表示する設定とページ一覧を経由する設定の両方で確認する。
   空・消失・ZIP/PDFだけのフォルダは停止条件に従って飛ばし、Ctrl+↑↓は従来のツリー順を維持すること。
3. 動画を含む本ではnative動画の同じキーでも移れること。準備中の連打で追加入力が積まれず、
   ★固定中・F12別窓では短い通知だけでmainの一覧を動かさないこと。

lib全件・workspace/vendor/docのfull gate、実GPU/実native presenter・実EPUB変換器を使う確認は未実施。
ClaudeCode側の後続検証へ引き継ぐ。

### 変更ファイル

- 実装: `src/app.rs`、`src/app/list_order_navigation.rs` (新規)、`src/app/collection_grid.rs`、
  `src/app/native_video.rs`、`src/app/viewer_context_registry.rs`、`src/keymap.rs`、
  `src/ui_fullscreen.rs`、`src/ui_main.rs`、`src/ui_dialogs/context_shortcuts.rs`。
- 既存EdgeのPage分類と一時intentの空初期化: `src/app/book_composition_permission.rs`、
  `src/app/collection_navigation.rs`、`src/ui_conceal.rs`、`src/ui_crop.rs`、`src/ui_erase.rs`、
  `src/ui_sns_split.rs`、`src/ui_fullscreen/tests/still_seek_menu.rs`。
- 新規test module: `src/app/list_order_tests.rs`、`src/app/list_order_virtual_tests.rs`、
  `src/app/list_order_worker_tests.rs`。上記既存実装fileにも回帰を追加した。
- 文書/生成物: `docs/README.md`、`docs/architecture-overview.md`、`docs/async-architecture.md`、
  `docs/detached-rework-plan.md`、`docs/fullscreen-navigation-consistency.md`、
  `docs/key-customization-impl-plan.md`、`docs/keymap-spec.md`、`docs/keymap.ini.default`、本書、
  `docs/next-release-backlog.md`、`docs/spec.md`、`htdocs/mimageviewer/index.html`、
  `htdocs/mimageviewer/manual/fullscreen.html`、`htdocs/mimageviewer/manual/shortcuts.html`。
  CHANGELOG・版番号・build/test scriptは変更しない。
