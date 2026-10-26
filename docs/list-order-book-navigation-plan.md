# 一覧の表示順で前後の本へ移る操作 (§1.444)

2026-10-26 第 2 版 (ClaudeCode)。設計・作業指示・検収 = ClaudeCode、実装 = Codex、独立レビューは通常どおり。
v4.9.0 の候補。コードの事実は 2026-10-26 時点の master (`7dfc52b2f`) を読んだもので、実行時の観測は無い。

- 第 1 版 (`7dfc52b2f`) は独立レビュー (Codex gpt-6.1-sol xhigh) で REJECT。指摘 7 件 (P1 2・P2 5) への対応を §9 に対照表で置く。
- 2026-10-26 利用者決定: **Ctrl+↑↓ (フォルダの木の順) は変えず、別操作にする**。一覧の順は「その一覧の中」しか
  たどれず、サブフォルダへ降りられないため、既定を置き換えると今の使い方ができなくなる。

## 1. 目的と前提

- 報告 (5ch >>613・>>617、用途は報告者に確認済み): スマートフィルタで絞り込み、日付の逆順に並べたサムネイル一覧で、
  **表示されている本 (ZIP / フォルダ等) を、表示されている順に次々と開いて確認したい**。
- 既存の操作はこの用途に合わない (コード確認):
  - `FsCtrlNavPrev/Next` (Ctrl+↑↓) は `handle_fullscreen_ctrl_nav_context` (`src/ui_fullscreen.rs` 約 42078) から
    `start_folder_nav` (`src/app.rs` 約 57675) の **フォルダの木の DFS** へ進む。順序は `folder_tree_sort_order` で、
    一覧の並べ替え・絞り込みとは別。
  - `FsSiblingPrev/Next` (Ctrl+PageUp/Down) は同じ親の兄弟をたどる別操作。
- 本を開くと、一覧の `items` / `visible_indices` / `details_order` は本のページで置き換わり、親の一覧の順序は残らない。
- 前例 (この設計の土台):
  - **`FolderNavMode::SmartFolder { state, fullscreen }`** (`src/app.rs` 約 4491): 入力時点の snapshot を mode に持たせ、
    worker が UI state を読まずに行き先を決める。行き先が決まった後の事前走査 (`scan_directory_with_visibility_cancel`)・
    `apply_folder_nav_result`・採用は DFS と共通。
  - ★固定 (`activate_snapshot`): 開いた時点の一覧を凍結した列として持つ。

## 2. 操作

- `KeyAction::FsListOrderPrev` / `FsListOrderNext` を追加する。説明「一覧の表示順で前の本を開く / 次の本を開く」。
- 文脈は `KeyContext::FsCommon` (フルスクリーン) だけ。一覧 (Grid) 側の同等操作は足さない。
- **既定キー: Ctrl+Shift+↑ / Ctrl+Shift+↓**。既定の割り当てとの衝突なし (レビューで確認。修飾キーは完全一致なので
  Ctrl+↑↓・Shift+↑↓ の音量操作とも区別される: `src/keymap.rs` 約 1190・1238)。
- `press_multiplicity` は Ctrl+↑↓ と同じ `EachPhysicalPress`。新しいゲームパッド割り当ては足さない。
- 既存の Ctrl+↑↓・Ctrl+PageUp/Down の挙動は変えない。
- 入力の gate (modal・入力欄・`viewer_mutation_modal_active`・表示待ち lock) は `FsCtrlNavPrev/Next` と同じものを通す。

## 3. 状態: 凍結した列

### 3.1 型と持ち主

```rust
struct ListOrderSnapshot {
    owner: ViewerContextId,               // 取得した時点の main context
    entries: Arc<[ListOrderEntry]>,       // 一覧の表示順。凍結
}
struct ListOrderEntry { path: PathBuf, kind: ListOrderEntryKind } // Folder / Zip / Pdf / ConvertibleArchive
struct ListOrderState { snapshot: ListOrderSnapshot, index: usize } // index = 今の本に当たる行
```

- `Option<ListOrderState>` を **`ViewerContextBundle` の 1 field** にする (context が持つ状態なので、park / mount で
  context と一緒に動く)。App 全体の bool や別の Option を増やさない。
- **今の本の位置は path ではなく `index` で持つ**。行が通常フォルダで、中の ZIP に着地した場合 (§4.3) も、列の上の位置は行のまま。
- 使える条件 (1 つの述語 `list_order_nav_available()` にまとめる):
  1. `ListOrderState` がある
  2. `snapshot.owner` が registry の現在の main id と一致し、mount 中の context がその main である
  3. `viewer_presentation == ViewerPresentation::MainWindow` (F12 linked で別窓に出しているときは使わない)
  4. detached / 独立 session でない
  満たさないときは「一覧から開いた本でだけ使えます」と短く知らせて何もしない。DFS へ黙って切り替えない。

### 3.2 取得するもの

- `current_grid_order()` (`src/app.rs` 約 72808。詳細表示なら `details_order`、それ以外は `visible_indices`) の順で、
  `GridItem::Folder` / `ZipFile` / `PdfFile` (EPUB を含む) / `ConvertibleArchive` の行だけを写す。画像・動画などの行は入れない。
  絞り込み (スマートフィルタ・ファセット・Ctrl+F・評価フィルタ) と並べ替えは `current_grid_order()` が既に反映している。
- 取った後の一覧の変更は反映しない (凍結)。live 追従は作らない (§7)。

### 3.3 取得する入口

- **main の通常フォルダの一覧から、本の行を開く要求を作るとき**。対象は次の 3 種の要求で、取得は 1 つの helper
  `main_folder_list_order_capture(idx) -> Option<ListOrderCapture>` に集め、各要求生成から呼ぶ:
  - 通常フォルダ (`AddressBarNav::Direct`、`grid_physical_navigation` `src/app/collection_grid.rs` 約 1355)
  - ZIP / PDF (`AddressBarNav::GridVirtual` の `GridVirtualOpenSource::Direct`)
  - 変換対象の書庫 (`MainGridArchive`。`grid_physical_navigation` を通らない: `src/app.rs` 約 57428、`src/ui_main.rs` 約 16782、
    `src/app.rs` 約 59301)
- Enter・ダブルクリック・明示の「ページ表示で開く / ページ一覧で開く」・既存のゲームパッドの決定は、上の要求生成を通るので同じ規則になる。
  実装時に各入口がこの 3 種のどれを作るかを列挙して確かめる。
- helper は次のどれかなら `None` を返す (取らない): main context でない、表示が `MainWindow` でない、一覧が通常フォルダの物理一覧でない
  (評価一覧・スマートフォルダ・検索結果・閲覧履歴・コレクション・ブックマーク・★固定)、
  要求の source が `Collection` / `Rating` owner。
- 下位の汎用 load 関数では取らない (DFS・起動時 open・アドレスバー入力が混ざるため)。

## 4. 移動

### 4.1 行き先の決め方: 新しい `FolderNavMode`

- `FolderNavMode::ListOrder { snapshot: ListOrderSnapshot, from_index: usize }` を足す (フルスクリーン専用)。
  `start_folder_nav` → `spawn_folder_nav` の既存経路に乗せ、worker の中で行き先の決め方だけを分岐する (SmartFolder と同じ形)。
- worker: `from_index` から前 / 次へ 1 行ずつ見て、**Ctrl+↑↓ (フルスクリーン) が候補に使う停止条件**
  (`folder_should_stop_with_memo`、同じ `FolderTreeOptions` と `archive_policy`) を満たす最初の行を行き先にする。
  満たさない行 (中に開けるものが無いフォルダ、削除済み、読めない、方針で除外される変換対象) は飛ばす。列は有限なので必ず終わる。
  cancel token を各行で確かめる。
- 行き先が決まった後の事前走査は DFS と同じコード (`spawn_folder_nav` の `scanned` 部分) を通す。UI スレッドに同期走査を戻さない。
- `FolderNavResult` に「列の上の行き先 index」を足す (`ListOrder` のときだけ `Some`)。

第 1 版の「開けない行で止まる・自動で飛ばさない」は撤回する。空のフォルダが列の途中にあると、そこから先へ進めなくなるため。
飛ばす基準は Ctrl+↑↓ と同じ停止条件で、新しい基準は作らない。

### 4.2 連打

- `ListOrder` の移動が進行中 (`folder_nav_pending` が `ListOrder`) の押下は**受け付けない** (v1)。
  `folder_nav_mode_same_kind` の累積 (`MAX_PENDING_NAV` = 符号付き最大 5) には乗せない。
  理由: 累積の継続 (`chain_folder_nav_if_pending`) は完了時の mode を複製して次を始めるので、`from_index` が古いまま残る。
  累積を正しく扱うには次の行き先を列から選び直す持ち主が要り、組み合わせが増える。
- 別の mode (DFS 等) が進行中に押したときは、既存の規則 (古い方を cancel して新しく始める) に従う。逆も同じ。
- 表示待ち lock 中の扱いは `inflight_fullscreen_folder_nav_mode` の既存規則に揃え、`ListOrder` は「進行中なら受け付けない」とする。

### 4.3 着地

- 採用・再オープン・ZIP / PDF の列挙待ち・hydration 後の再オープン・開始ページ (`book_nav_resume`) は、
  **同じ行き先へ Ctrl+↑↓ (`FolderNavMode::Fullscreen`) で着いたときと同じ** にする。`apply_folder_nav_result` の
  `Fullscreen` の後処理を共有し、新しい開き方を作らない。
- 通常フォルダの行で、中に直接表示できる画像が無く ZIP / PDF だけがあるときは、既存の再オープンが中の先頭の書庫を開く
  (`src/app.rs` 約 59335・60188)。これも Ctrl+↑↓ と同じで許容する。列の位置は §3.1 のとおり行の `index` のまま。
- 端 (先頭の前・末尾の次、または残りの行がすべて停止条件を満たさない): 既存の `FsBoundaryHint::Edge` の形で
  「一覧の先頭です / 一覧の末尾です」と知らせて止まる。ループしない。
- フルスクリーンは保つ。閉じる・戻る (Esc / BS) は**既存の規則と設定のまま** (Backspace はページ一覧へ、Esc は条件により親一覧へ)。
  第 1 版の「最後に開いた本の行を選んで一覧へ戻る」と、移動ごとの `save_leaving_folder_grid_position` の呼び出しは撤回する
  (この関数はその時点のフォルダの scroll / 選択を保存するもので、親一覧の選択を決めるものではない)。親へ戻ったときの選択は
  既存の `effective_folder()` の名前による選択要求に任せる。中の書庫に着地していた場合は、その書庫の親 (= 行のフォルダ) へ戻る。
- 採用前の失敗 (worker の cancel・走査失敗) は既存の準備経路で止まり、状態を変えない。採用後のまれな失敗は既存のログ + 通知で
  割り切り、専用の巻き戻しは作らない (CLAUDE.md「まれな失敗は既定で割り切る」)。

## 5. 状態遷移 (保持・交換・破棄を要求の由来で決める)

採用の口は `adopt_main_list_navigation_with_completion` (`src/app.rs` 約 24944) に集まっている。`MainListAdoption` に
**既定値の無い** field `list_order: ListOrderAdoption` を足し、全ての構築箇所 (2026-10-26 時点で約 19 か所) に由来を明示させる
(書き忘れはコンパイルで止まる)。状態を書き換えるのはこの採用の口だけ。

```rust
enum ListOrderAdoption {
    Install(ListOrderSnapshot, usize), // 一覧から本を開く要求の採用
    Advance(usize),                    // ListOrder 移動の採用
    Keep,                              // 今の場所の再読込・再投影
    Clear,                             // それ以外の場所の移動
}
```

| 出来事 | 列 |
| --- | --- |
| §3.3 の要求が採用された | `Install` (新しい列と開いた行の index) |
| §3.3 の要求が採用前に cancel / 失敗 / 別の要求に置き換え | 変えない (取得候補は要求に付いたまま一緒に捨てられる) |
| `ListOrder` 移動が採用された | `Advance` (index を行き先へ) |
| `ListOrder` 移動が採用前に cancel / 失敗、端で止まった | 変えない |
| DFS・兄弟移動・履歴の戻る / 進む・アドレスバー・ブックマーク・親一覧へ戻る・評価 / スマートフォルダ / 検索 / コレクション / 閲覧履歴への移動の採用 | `Clear` |
| 今の場所の再読込・ZIP の更新反映 (`reconcile_zip_refresh_adoption`)・ファセットの再投影・hydration 完了 | `Keep` |
| フルスクリーンを閉じて同じ本のページ一覧へ、同じ本の中で開き直す | 採用が起きないので変わらない |
| F12 で同じ context を別窓へ出す / 戻す (linked) | 変わらない (別窓の間は §3.1 の 3 で使えない) |
| context の park / mount | bundle と一緒に動くので変わらない |
| fork の子 context | 空 (bundle の既定値) |
| promote | 新しい main は空 (`ReplaceProjectionWithFreshEmpty`)。旧 main の payload に残った列は `owner` が main id と一致しなくなるので、§3.1 の 2 で二度と使えない |

- 起動時の open・コマンドライン・D&D・DFS で入った本は `Install` されないので、列を持たない (§3.1 の通知になる)。
- 一覧へ戻って別の本を開くと、戻る採用で `Clear`、開く採用で `Install` になる。

## 6. 範囲外 (v1) と理由

- **一覧側 (Grid) の同等操作**: 一覧に居るときは見えている順に選んで開けばよい。
- **detached・複数ウィンドウ・F12 linked の別窓での使用**: §3.1 の述語で通知だけにする。列を別 context に渡す設計は、
  F12 往復・promote・park の組み合わせが増えるので v1 では作らない。
- **評価一覧・スマートフォルダ・コレクション・検索結果・閲覧履歴**: それぞれ既存の表示順ナビゲーションや検索の範囲がある。
- **新しいゲームパッド・リング操作**: キーボード操作の要望のため。

## 7. 検討した単純化 (CLAUDE.md「設計の簡素化」)

- **凍結した列**: 一覧の変更に追従させると、絞り込み変更・削除・並べ替え・再読込と移動中の組み合わせをすべて扱う必要がある。
  凍結なら取得と破棄の時点だけを扱えばよい。報告者の用途 (絞り込んで順に見ていく) は凍結で満たせる。採用。
- **既定の Ctrl+↑↓ を置き換える案**: 利用者と検討し、サブフォルダへ降りられなくなる (今の使い方ができなくなる) ので不採用 (冒頭)。
- **状態を書き換える口を採用の 1 か所にし、由来を型で必須にする**: パスの変化から推測する案 (第 1 版) は、移動の種類と
  取消・置換の組み合わせを取りこぼす。採用。
- **既存の `FolderNavMode` に 1 variant 足す**: 走査・事前走査・採用・再オープン・ZIP / PDF の待ちを DFS と共有できる。
  独立の移動経路を作る案は不採用。
- **連打を受け付けない**: 累積に乗せると行き先を選び直す持ち主が要る。v1 は 1 回ずつ。採用。
- **飛ばす基準を Ctrl+↑↓ と同じにする**: 新しい「本として開けるか」の判定を作らない。採用。
- **戻り方を既存のまま**: 一覧の位置を覚え直す専用処理を作らない。採用。
- **F12 linked・detached では使わない**: `owner` id と表示先の確認だけで済み、registry の遷移に手を入れない。採用。

## 8. 変更範囲・テスト・文書

### 8.1 変更範囲

- `src/keymap.rs`: `KeyAction` の 2 variant、`ini_name()` / `description` / `context()` / `trigger()` / `press_multiplicity` /
  `default_chords()` / `ALL_ACTIONS` などの各表。egui 側の押下数処理。
- native 動画: whitelist は `FsCommon` を拾うが、App 側の resolver / dispatcher は既存 4 操作を明示列挙している
  (`src/app/native_video.rs` 約 12571・12978)。2 操作を足す。動画ショートカットのヘルプにも足す。
- `src/ui_fullscreen.rs`: `FsCommon` の dispatch。gate は `FsCtrlNavPrev/Next` と同じ。
- `src/app.rs`: `FolderNavMode::ListOrder`、worker の分岐、`FolderNavResult` の行き先 index、`apply_folder_nav_result` の共有、
  `MainListAdoption::list_order` と全構築箇所、§3.3 の helper と 3 種の要求への受け渡し。
- `src/app/viewer_context_registry.rs`: `ViewerContextBundle` に `list_order: Option<ListOrderState>`。名前が `pending` で
  終わらないので `context_async_owner_registry_audits_bundle_pending_fields` の登録対象ではない (非同期の持ち主ではない)。
- `docs/keymap.ini.default` は生成物として再生成する。

### 8.2 detached 凍結規則 (CLAUDE.md・`docs/detached-rework-plan.md` §2)

`ViewerContextBundle` への field 追加と、F12 linked の表示先判定は detached の述語 / context 経路に触れる。
- ClaudeCode の立場: **症状パッチではなく構造的な修正**。context が所有する状態を bundle の 1 field として持ち、
  registry の遷移 (park / mount / fork / promote) には手を入れず、使える条件を owner id と表示先で判定するだけである。
  guard・delay・retry・一括 reset・silent fallback を足さない。
- 独立レビューで、この主張への**明示の合意**を取る。合意できたら、触れた範囲と理由を `docs/detached-rework-plan.md` §11 に記録する。

### 8.3 テスト (handler-level と状態遷移)

- 取得: スマートフィルタ・ファセット・Ctrl+F・評価フィルタ・詳細表示の並べ替え (昇降順)・名前 / 日付の昇降順で、列が画面の順・対象と一致する。
  画像・動画の行は入らない。§3.3 の 3 種の要求で取れる。評価一覧・スマートフォルダ・検索・閲覧履歴・コレクション・
  Collection / Rating owner・detached・F12 linked では取らない。
- 遷移: §5 の表の各行 (特に、採用前の cancel / 置換で列が変わらないこと、`Keep` の各出来事で列が残ること、
  DFS・履歴・親へ戻るで消えること、fork の子が空、promote 後に旧 payload の列が使えないこと、別 context の bundle が変わらないこと)。
- 移動: 前後・端の通知・停止条件を満たさない行を飛ばす・残りが全部満たさないとき端の通知・中の書庫への着地と、その後の次の行・
  直接ページ表示とページ一覧の両方・`book_nav_resume` の有無・進行中の押下を受け付けない・DFS 進行中の押下で DFS が cancel される。
- 入力: 既定キー・変更した割り当て・repeat・入力欄 / modal 中・使えないときの通知・既存の Ctrl+↑↓ / 兄弟移動が変わらない・
  native 動画再生中に効く。
- keymap: 生成した `.default` と `docs/keymap.ini.default` の一致テスト。
- 実アプリ: 隔離 data-dir で、スマートフィルタ + 日付降順の本の一覧から順に開く (ClaudeCode が PC 使用の了承を得て行う)。

### 8.4 文書

- `docs/keymap-spec.md` のフルスクリーンの表、`docs/fullscreen-navigation-consistency.md` §2・§3、
  マニュアル (`fullscreen.html`・`shortcuts.html`) に新しい操作と「一覧から開いた本でだけ使える」こと、
  Ctrl+↑↓ との違い (一覧の中だけ・サブフォルダへは降りない) を書く。

## 9. 第 1 版レビュー指摘への対応

| 指摘 | 対応 |
| --- | --- |
| P1-1 維持・破棄をパス変化で決めている | §5: 採用の口 1 か所で、由来を既定値の無い型で必須にする。遷移表 |
| P1-2 `apply_folder_nav_result` は目的地の差し替えができない | §4.1: `FolderNavMode::ListOrder` で worker の行き先決定だけ分岐し、事前走査・採用は共有。§4.2: 連打は受け付けない |
| P2-3 変換対象の書庫が `grid_physical_navigation` を通らない | §3.3: 共通 helper を 3 種の要求生成から呼ぶ |
| P2-4 開き方・戻り方の説明が既存仕様と衝突 | §4.3: Ctrl+↑↓ と同じ着地、戻りは既存のまま。保存関数の呼び出しを撤回 |
| P2-5 Folder 行の定義と anchor | §4.1: Ctrl+↑↓ と同じ停止条件で飛ばす。§3.1・§4.3: 位置は index、中の書庫への着地を許容 |
| P2-6 main と F12 linked の混同、promote | §3.1: owner id と表示先の述語。§5: fork / promote の行。§8.2: 凍結規則の合意と記録 |
| P2-7 keymap の表だけでは native 動画で動かない | §8.1・§8.3: resolver / dispatcher・ヘルプ・`.default` 一致テスト |
