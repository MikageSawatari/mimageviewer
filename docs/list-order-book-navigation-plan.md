# 一覧の表示順で前後の本へ移る操作 (§1.444)

2026-10-26 第 3 版 (ClaudeCode)。設計・作業指示・検収 = ClaudeCode、実装 = Codex、独立レビューは通常どおり。
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

## 3. 状態: 最後に本を開いた一覧の、凍結した列

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
- 開く操作が取消・失敗しても、列は「いま見ている一覧」の写しなので害は無い。次に本を開けば、また置き換わる。

### 3.4 使える条件と今の位置 (毎回求める)

述語 `list_order_position() -> Option<(Arc<ListOrderSnapshot>, usize)>` を 1 つだけ作り、操作の受付で毎回呼ぶ。次をすべて満たすとき位置を返す:

1. 列がある
2. `snapshot.owner` が registry の現在の main id と一致し、mount 中の context がその main である
   (promote で main id は新しく発行され、旧 id が main に戻ることは無いので、旧 main に残った列は二度と使えない)
3. 表示先が `ViewerPresentation::MainWindow` か `Fullscreen` (通常の専用フルスクリーン。App の初期値) で、
   `DetachedWindow` でも独立 owner でもない
4. 一覧の surface が `TopLevelGridSurface::Folder` で、★固定が無効
5. 表示中の本 (`effective_folder()`) のパスが `entries` のどれかの `path` と一致する (一致比較は `folder_tree::path_eq`)。その位置を返す

満たさないときは「一覧から開いた本で使えます」と短く知らせて何もしない。DFS へ黙って切り替えない。

- 5 の帰結: 一覧から開いた本・この操作で移った本はもちろん、DFS や履歴で**同じ列に含まれる本**へ来たときも使える。
  列は「最後に本を開いた一覧」の写しなので、そこに含まれる本なら前後が決まる。列に無い場所へ移ると自然に使えなくなり、
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
- この操作の準備中に Ctrl+↑↓ を押した場合、Ctrl+↑↓ の準備中にこの操作を押した場合も、同じ gate で拒否される。cancel して乗り換える処理は作らない。
- 累積 (`folder_nav_mode_same_kind` と `MAX_PENDING_NAV`) には乗せない。gate が準備中の押下を拒否するので、累積が起きる経路が無い。
  `folder_nav_mode_same_kind` は `ListOrder` を他のどの mode とも同じ種類と判定しない。

### 4.3 着地

- 採用・再オープン・ZIP / PDF の列挙待ち・hydration 後の再オープン・開始ページ (`book_nav_resume`) は、
  **同じ行き先へ Ctrl+↑↓ (`FolderNavMode::Fullscreen`) で着いたときと同じ**にする。`apply_folder_nav_result` の
  `Fullscreen` の後処理と、ZIP / PDF が `PhysicalHistoryTransition` を経て後から採用される経路を共有し、新しい開き方を作らない。
  `ListOrder` が `Fullscreen` と同じに扱われる必要のある分岐 (`StartupListIntent::folder_navigation`・`history_trigger`・`perf_tag`・
  `inflight_fullscreen_folder_nav_mode` など) は、実装時に `FolderNavMode` の match を全部たどって揃える。
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
| この操作で移動した (ZIP / PDF の後からの採用・内側の書庫・hydration を含む) | 変えない | 着いた本が列にあるので使える |
| DFS・兄弟移動・履歴・アドレスバー・ブックマークで移った | 変えない | 着いた本が列にあれば使える。無ければ通知 |
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
  - 引き換え: DFS や履歴で列に含まれる本へ来たときも使える。列は「最後に本を開いた一覧」なので前後は決まり、害は無いと判断した。
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
  第 3 版はこの条件に合わせた (§3.1・§3.4 の 3)。第 3 版の再レビューでも合意を確かめる。
- 合意が保たれたら、触れた範囲と理由を `docs/detached-rework-plan.md` §11 に記録する。

### 8.3 テスト (handler-level と状態遷移)

- 取得: スマートフィルタ・ファセット・Ctrl+F・評価フィルタ・詳細表示の並べ替え (昇降順)・名前 / 日付の昇降順で、列が画面の順・対象と一致する。
  画像・動画の行は入らない。§3.3 の 3 か所 (通常フォルダ・ZIP / PDF・変換対象) で、load の開始前に取れる。
  評価一覧・スマートフォルダ・検索・閲覧履歴・コレクション・サブフォルダ展開・★固定・Collection / Rating owner・main 以外では取らず、既存の列も変えない。
- 位置 (§3.4): 一覧から開いた本・この操作で移った本 (通常フォルダ・ZIP・cold PDF・warm PDF・変換対象)・DFS で列内の本へ来たときに位置が出る。
  列外の本・★固定中・`DetachedWindow`・fork の子・promote 後の旧 context では出ない。`MainWindow` と `Fullscreen` の両方で出る。
  別 context の bundle が変わらない。
- 移動: 前後・端の通知・停止条件で飛ばす行 (ZIP / PDF だけのフォルダ・空のフォルダ・消えた行)・残りが全部飛ばす行のとき端の通知・
  直接ページ表示とページ一覧の両方・`book_nav_resume` の有無。
- gate: 準備中 (sequence・hydration・表示待ちを含む) の押下を受け付けない。Ctrl+↑↓ の準備中にこの操作、この操作の準備中に Ctrl+↑↓ を押しても乗り換えない。
- 入力: 既定キー・変更した割り当て・repeat・入力欄 / modal 中・使えないときの通知・既存の Ctrl+↑↓ / 兄弟移動が変わらない・
  native 動画再生中に効く。
- keymap: 生成した `.default` と `docs/keymap.ini.default` の一致テスト。
- 実アプリ: 隔離 data-dir で、スマートフィルタ + 日付降順の本の一覧から順に開く (ClaudeCode が PC 使用の了承を得て行う)。

### 8.4 文書

- `docs/keymap-spec.md` のフルスクリーンの表、`docs/fullscreen-navigation-consistency.md` §2・§3、
  マニュアル (`fullscreen.html`・`shortcuts.html`) に新しい操作、「最後に本を開いた一覧の順で移る」こと、
  Ctrl+↑↓ との違い (一覧の中だけ・サブフォルダへは降りない) を書く。

## 9. レビュー指摘への対応

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
