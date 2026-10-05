# §1.297 一覧先頭の「..」による親ナビゲーション設計案

作成日: 2026-10-04。調査基準: `next-list-ui` / `ceec8d5d2`。
状態: **見送り (2026-10-04 利用者判断)**。再検討時の資料として残す。理由は backlog-on-hold.md §1.297。
本段の変更は本書だけ。製品コード、既存文書、コミット、ビルド、アプリ起動は対象外。
以下の現状はソースを読んだ事実であり、実アプリでの観測ではない。行番号は調査基準 HEAD のもの。

## 1. 要件・範囲・不変条件

正本は [backlog-on-hold.md §1.297](backlog-on-hold.md)。
5ch >>475 の「矢印キーと Enter だけでフォルダ間を往復する」に対応する。
設定は既定 OFF。ON のとき対象一覧の先頭に選択可能な `..` を置き、Enter / クリックで既存の親移動を実行する。
[backlog-on-hold.md §1.298](backlog-on-hold.md) の**余白ダブルクリックで親へ戻る操作は対象外**。

守る条件:

- `..` はナビゲーション専用。実ファイル・フォルダ、ページ、スタックメンバーではない。
- ソート・絞り込み・件数・チェック・★・タグ・補正・削除・コピー・移動・製本・D&D・Remote のデータへ混入しない。
- 親移動先・ZIP 内の階層・元フォルダを選ぶ復帰・履歴・非同期ロード・別窓保存は既存経路が決める。`Path::join("..")` 等による別ロジックを作らない。
- OFF では既存の並び、選択、クリック方式、カーソルループ、行スナップ、空表示を維持する。
- 別窓が開いていても main 一覧は操作できる。Parentの表示・選択・設定切替だけで別窓を閉じたり操作を制限したりしない。
  親移動実行に伴う既存のowning session終了・退避は維持し、無関係なsibling contextは変更しない。

参照した規則: `CLAUDE.md` の必読、設計の簡素化、バグ修正、通常操作の退行相談、keymap、UI/スクロール、UI 応答性。
加えて `docs/README.md`、`architecture-overview.md`、`keymap-spec.md`、`key-customization-impl-plan.md`、
`virtual-folders.md`、`filename-stack-plan.md`、`top-level-grid-view.md`、`detached-rework-plan.md` §2、
`ui-responsiveness.md` §4、`preferences-layout-guidelines.md`、`development-build-and-test.md`。

## 2. 現状のコード事実と index の境界

### 2.1 一覧・カーソル・非同期処理

`raw index` は `items` の添字、`display position` は表示順の位置。既に二つは異なる。
「先頭に 1 を足す」を raw index に適用すると、対応配列・worker・復帰位置まで変わる。

| 所有 / 処理 | 現状の参照点 (file:line) | index に依存する内容 |
| --- | --- | --- |
| 実項目・サムネイル・選択 | `src/app.rs:13152`, `:13201`, `:13202` | `items: Vec<GridItem>`、`thumbnails`、`selected: Option<usize>`。選択は raw index |
| チェック / 表示集合 / 詳細順 | `src/app.rs:13900`, `:14353`, `:14356` | `checked: HashSet<usize>`、`visible_indices: Vec<usize>`、`details_order: Vec<usize>` は raw index を保持 |
| 表示順の取得 | `src/app.rs:62533`, `:62547` | `current_grid_order()` は詳細順または可視順。読書用 `current_reader_order()` は Collection root 等で別契約 |
| フィルタ・選択補正 | `src/app.rs:61443`, `:63191`, `:63210` | 実項目を列挙して可視集合を作る。隠れた選択を直前優先で可視項目へ補正、未選択なら先頭実項目を選ぶ |
| 矢印 / Home / Page / Shift 範囲 | `src/app.rs:47377`, `:47413`, `:47440` | 表示順を複製し、選択 raw index の表示位置を検索して移動先を raw index へ戻す。Shift は表示位置区間内の checkable 実項目を追加 |
| ループ純関数 | `src/app.rs:12907` | `grid_cursor_nav_target_pos` は位置・件数・列数で計算。左右は線形、上下は列数単位。ON の上下ループは列を保つ |
| Enter / Space | `src/app.rs:47292`, `:47468`, `:47589` 以降 | `GridOpenSelected` は実項目の open 分岐へ。現在の移動・open は `vi_len > 0` の内側なので、親だけの一覧を扱う変更が必要 |
| マウス・範囲選択・D&D | `src/ui_main.rs:15444`, `:15537`, `:15551`, `:15571`, `:15744` | セル ID、クリック anchor、同一 generation の click pairing、チェック、open / stack / detached へ raw index を渡す |
| 余白の選択解除 | `src/ui_main.rs:16019` | Explorer 方式の背景クリックは選択・anchor・チェックを解除。Check 方式は維持。余白に親移動は無い |
| グリッド仮想描画 | `src/ui_main.rs:18313`, `:18622`, `:18657` | 可視件数から総行数、`row * cols + col` から可視順を引き、実 idx のバッジ・チェック・描画へ渡す |
| 詳細表示 | `src/ui_main.rs:16187`, `:16287`, `:16311`, `:17734` | `details_order` を行へ投影。可視行・前後8行のタグ prewarm・scroll hint・プレビュー・選択枠が idx と対応 |
| スクロール / 下部情報のレイアウト | `src/app.rs:50375`; `src/ui_main.rs:16264`, `:19161` | 選択の表示位置から行を計算し、行境界に snap。下部情報欄も詳細一覧の高さから gutter を予測 |
| サムネ要求 / 結果 | `src/app.rs:45649`, `:44909`; `src/thumb_loader.rs:336`, `:425` | `update_keep_range_and_requests`、`LoadRequest.idx/items_gen`、`ThumbMsg.idx/items_gen`、`requested` 等が実 idx を使う |
| 対応データ・失効 | `src/app.rs:35207`, `:37102` | 並べ替え・差し替えは items/meta/thumb と idx-keyed rating/rotation/adjustment/AI/cache を揃えて世代更新 |
| 件数・選択情報 | `src/ui_main.rs:12184`, `:13812`, `:19155`; `src/app.rs:60222` | 可視実件数を表示。選択情報と lazy metadata target は selected idx から解決 |
| 最後の画像 | `src/app.rs:80588` | 選択実画像を `last_selected_thumb_sample` へ保存。親を選んでもこれを疑似画像に置き換えない |
| 削除 / 他の項目操作 | `src/ui_dialogs/context_menu.rs:2274`, `:2298`, `:2310`; `src/app.rs:65651`, `:78303` | checked 優先、無ければ selected。実パス・kind・補正等へ解決する。親を raw index にすると危険 |
| 別 context | `src/app/viewer_context_registry.rs:740`, `:755`, `:916`; `src/app.rs:3316`, `:4089` | bundle / 復元状態も items、可視順、selected、scroll を保持。App の mounted projection だけ変える設計は不可 |
| スタック | `src/filename_stack_ui.rs:261`, `:270`, `:773`, `:1138`, `:1886` | items と meta の同長条件、集約/flat の materialize、idx の再選択・swap。`..` はどちらにも入れない |
| ゲームパッド | `src/app/gamepad_input.rs:6770`, `:7234` | キーボードと別の表示位置→selected / accept 経路。詳細の左右はページ単位で、キーボード左右と同一ではない |
| Remote | `src/remote_ipc/container.rs:4675`, `:4687`, `:4702` | Remote は独自に物理一覧を再計算し address-based entry / page を作る。PC 描画位置の番号を公開する仕組みではない |

探索では `self.selected` の参照が src 内で 146 行あった（テスト等の `app.selected` は別）。
したがって B 案も「描画に一行足すだけ」ではない。selected の所有型変更と raw-index accessor の通しが主要費用。
上表は境界の inventory であり全呼び出しの全行一覧ではない。実装では selected 読み書き、snapshot/restore、項目操作の fallback を機械検索して移行漏れをなくす。

### 2.2 既存の親移動は一つの判定だけではない

| 入口・判定 | 現状のコードと意味 |
| --- | --- |
| Backspace / Alt+↑ | `src/keymap.rs:6476` の `GridParentFolder`。`src/app.rs:47799` から snapshot → Ctrl+G → Ctrl+S → tag → rating → Ctrl+F 起点禁止 → ZIP 内 back → `resolve_grid_parent_nav()` の順 |
| 項目入力が許されない場合のキー経路 | `src/app.rs:46928` にも親 Action の処理がある。通常の項目移動より前に処理する独立入口であり、後段だけを変更して完了しない |
| フォルダバー ↑ | `src/ui_main.rs:13761`, `:14107`。検索には別の drill-up、通常側は target の有無と snapshot / Ctrl+F 禁止を併用。ZIP back を先に実行してから resolver |
| Pad B / ring / gesture | `src/app/gamepad_input.rs:7356`, `:6069`。独自の前処理後に ZIP back / resolver。tag・rating・detached の前処理は通常キーと完全同一ではない |
| 読み取り専用 target | `src/app.rs:20447` の `grid_parent_nav_target()`。Collection / bookmark / history / rating / sub展開 / smart を先に判定し、その後 effective path の親。**この戻り値が Some だけでは許可を意味しない** |
| 実行時 resolver | `src/app.rs:20483`。通常親なら `select_after_load` に元フォルダ名を入れ、ドライブルートなら `DriveList(Some(origin))` を返す |
| effective path | `src/app.rs:20053`。ドライブ一覧は None、変換書庫では元アーカイブを使用。cache ZIP の親へ戻さない |
| Ctrl+F 起点禁止 | `src/app.rs:20710` の `local_search_blocks_parent_nav()`。filter / pending と origin が現在地と一致する場合だけ禁止。子へ入った後は通常親移動可 |
| Collection / rating | `src/app/collection_grid.rs:1118` は PhysicalSource から collection restore、root は None。`src/app.rs:30074`, `:30078` は root / pending でも戻れるが、意味は一覧の終了・復帰 |
| sub展開 / smart | `src/app/subfolder_expansion.rs:1888` は saved folder / root へ戻す（物理親ではない）。`src/app/smart_folder.rs:8032` は scoped parent または synthetic root へ戻す |
| 履歴 / bookmark | `src/app.rs:20108`, `:20124` は由来一覧へ復帰。ネストZIP深度の扱いも同一ではない |
| 最終適用 | `src/app.rs:85602`, `:85652` の input / address nav 合流点。Direct / GridVirtual / DriveList / Collection 等を既存 owner で処理 |

現状には全入口の可否・復帰意味を一度に答える完成済みの関数はない。これは要件との矛盾ではなく、
既存 target と禁止条件を**同じ読み取り専用判定へまとめる必要がある**という調査結果。
本機能の表示可否に `effective_folder().parent().is_some()` だけを使うことや、Pad B をそのまま呼ぶことは避ける。
既存入口の差異を本段で不具合と断定したり、全部を新しい挙動に統一したりしない。

## 3. 実現方式の比較と推奨

| 案 | 状態・利点 | 影響・費用・判断 |
| --- | --- | --- |
| A: `items[0]` に `GridItem::ParentNav` を挿入 | カーソルは従来の `Option<usize>` で動く。全 variant match で明示的に除外できる | items/meta/thumb の長さと全 idx が変わる。ソート各経路で先頭固定、filter 各経路で保持、件数から除外、checkable・削除・移動・コピー・D&D・★・タグ・補正・製本・外部ツール・代表pinを拒否。Remote address 化、detached の物理項目構築、詳細 order / metadata / prewarm、stack 集約/flat、履歴・snapshot・世代・worker結果まで専用分岐が必要。旧 ZIP separator 撤去とも逆向き。最大費用・非推奨 |
| B: 実項目の外に先頭 1 マスを描画・入力投影 | raw index とデータの順序・世代は不変。カーソルだけ `None / Parent / Item(idx)` の単一 owner へ変更 | 表示位置⇔実 idx の薄い写像、スクロール、空表示、入力、selected consumer / bundle / restore の型移行が必要。データ・worker・Remote には疑似項目を持ち込まない。中〜大規模だが境界を限定できる。**推奨** |
| C: 一覧上部に独立の ↑ ボタン / 一行全幅領域 | データ index を変えずボタンを再利用可能 | 矢印からボタンへ focus を渡すには別の focus state / 上下境界 / scroll 例外が要る。ボタンだけでは要望を満たさず、全幅一行は grid で一行分を消費する。B の1マス方式より状態・表示費用が増えるので非推奨 |
| B の近道: `parent_selected: bool` を追加 / `usize::MAX` を選択に使う | 初期差分が小さく見える | `selected` と parent flag の矛盾、None の意味の重複、sentinel の実添字アクセスを増やす。採用しない |

### 3.1 状態の組み合わせを減らす具体策

- `..` は導出された一つの表示枠。items に挿入せず、別の親項目配列・疑似 metadata・別 thumbnail request は作らない。
- `GridCursor { None, Parent, Item(usize) }`（名称は仮）で現 `selected` の owner を**置換**する。`selected` と別 bool を併存させない。
- 実項目を要する consumer には `selected_item_index() -> Option<usize>` を渡す。Parent / None は None。
  `checked` は従来の実 idx 集合のまま。last-selected-image、fullscreen_idx、クリック範囲 anchor は別の役割なので実 idx / 実画像のまま。
- `current_grid_order()` / `current_reader_order()` は実項目だけを返す契約を維持する。
  ナビ用 `GridEntryLayout`（仮称）は既存 order と `has_parent` を借り、長さ・位置→entry・親による位置の加減算を O(1) で提供する。
  実 idx→表示位置の探索は既存 order の探索を使用し、逆引きcacheや新しい全件走査を追加しない。
  Parent を加えた全件 Vec を新規に構築しない。
- 長い処理を新たに modal にする案は採用しない。これは既存の短い親操作への入口追加であり、ロード中の入力規則は既存 owner が担当する。
  既存の変換・保存・password modal には従い、新 pending / rollback / resume state は足さない。
- 設定変更時に一覧を reload / 別窓を閉じる案も検討した。データが変わらない表示設定であり、reload は I/O・worker cancel・実選択再構築を増やす。
  既存の再描画と選択位置計算だけで済むため採用しない（§6）。

## 4. 表示場所と親移動の共有境界

### 4.1 判定と実行

既存 `grid_parent_nav_target()` と `local_search_blocks_parent_nav()` を中核に、
読み取り専用 `grid_parent_nav_availability()`（仮称）へ現在の一覧 owner / search / snapshot / ZIP depth の事実を集約する。
返答は「禁止」「物理親」「ZIP 内一段」「ドライブ一覧」「由来一覧へ復帰」などの排他的な意味分類。
移動先を新計算するのではなく、既存 target / ZIP nav / view owner の結果を分類する。
ZIP の下層は `effective_folder` が外側 ZIP のままなので、`zip_nav.at_root()` を必ず優先する。
sub展開・smart は Direct を返すことがあるため、AddressBarNav の variant だけで物理親と決めない。

`parent_row_visible = settings.grid_show_parent_nav_row && availability.supports_parent_row()`。
同じ関数を grid / details / cursor / Home / scroll で使う。フォルダバーの通常側の可否も同じ基礎判定へ寄せ、
検索専用の drill-up、snapshot child のキー復帰等、既存入口固有の意味は保持する。
「バーが有効なら必ず `..` を表示」ではない。由来一覧へ戻る操作は引き続きバー / Backspace が担当する。

実行は通常キーの親分岐 (`src/app.rs:47799`) を副作用・優先順を保って helper に切り出し、
Backspace / Alt+↑ と `..` の Enter / 左クリックがそれを呼ぶ。
入力禁止時の早期経路 `:46928` も inventory に含め、既存の許される操作を失わない。
`..` の handler は新しい親 Path / select_after_load / load_folder を組み立てず、既存の resolver と nav 合流点へ渡す。
ZIP back がその場で処理される場合と、AddressBarNav を返す場合の両方を扱う。クリックの返答も render_grid の既存 nav 出力へ合流する。
描画時の target を保存して後で実行せず、入力受理時の現在 owner を再確認する。

### 4.2 推奨の表示表（設定 ON）

これは**新しい表示対象の提案**。既存 Backspace / ↑ の機能を削る表ではない。グリッドと詳細表示に同じ表を適用する。

| 場所 / 状態 | 既存の親操作 | grid / details の `..` 推奨 | 根拠・補足 |
| --- | --- | --- | --- |
| 通常の実フォルダ | 物理親へ、元フォルダを再選択 | 表示 / 表示 | resolver をそのまま使用 |
| ドライブルート | ドライブ一覧へ、元ドライブ選択 | 表示 / 表示 | 既存の「一段上」に合わせる。tooltip は「ドライブ一覧へ」。フォルダ親 Path が無いから非表示にはしない |
| UNC / 共有ルート | 既存 effective path の resolver が決める | 共通判定で許可される場合に表示 / 同左 | ドライブ判定や stat を追加しない。共有ルートを独自にドライブ一覧へ変更しない |
| ドライブ一覧 / current 無し | effective_folder は None | 非表示 / 非表示 | 親が無い |
| 通常 ZIP / CBZ root の一覧 | 書庫の親実フォルダへ | 表示 / 表示 | 本を出る既存動作。container / ページ idx は不変 |
| ZIP 内フォルダ / 入れ子 ZIP・RAR | zip_nav_back で一段戻る | 表示 / 表示 | tooltip は「書庫内の親へ」。実フォルダへ飛ばない |
| PDF / EPUB のページ一覧 | 元の本の親へ | 表示 / 表示 | PDF ページ数や page 0 に数えない。EPUB は論理パスを使用 |
| 直接閲覧 RAR / 変換 ZIP の一覧 | ZIP 階層 / effective 元書庫の親へ | 表示 / 表示 | cache ディレクトリは見せない |
| 画像のみフォルダの一覧 | 実フォルダの親へ | 表示 / 表示 | 自動fullscreen自体は変更しない。fullscreenに `..` は出さない |
| Ctrl+F の filter / pending の起点 | local_search_blocks_parent_nav が禁止 | 非表示 / 非表示 | filter による全件除外だけでは親の可否を決めない |
| Ctrl+F 起点から入った子 | origin と現在地が違えば親可 | 表示 / 表示 | 「検索バーがある」を一律禁止にしない |
| Ctrl+S / Ctrl+G / tag の結果 root | no-op / 仮想階層の根 | 非表示 / 非表示 | saved_folder の物理親を見せない |
| 上記検索からの drill / 物理子 | 既存 search stack を一段戻る | 非表示 / 非表示 | 今回は検索 owner 内全体を対象外。復帰の意味を `..` へ広げる案は§10 |
| ★固定 root / child | root は制限、child は snapshot list へ戻る経路あり | 非表示 / 非表示 | snapshot owner を保ち、既存キーとバーの差もそのまま |
| rating root / pending / physical child | 元の場所 / rating view へ復帰 | 非表示 / 非表示 | root でも resolver が Some になる。Some だけの表示判定は不可 |
| collection root / physical child | root には物理親なし、child は typed collection restore | 非表示 / 非表示 | コレクションの entry / revision / viewport anchor を保つ |
| 閲覧履歴 / bookmark root および由来復帰を持つ一覧 | 履歴 / bookmark / HistoryBack 等へ | 非表示 / 非表示 | 複数場所の集約、物理親と違う意味 |
| サブ展開 root / スタック付きサブ展開 | saved folder / 展開起点へ戻る | 非表示 / 非表示 | Direct でも物理親ではなく展開終了 |
| smart root / scoped drill / container child | entry root 内 / synthetic root へ戻る | 非表示 / 非表示 | scope 境界は既存 owner に委ねる。深い物理子だけを特別扱いしない |
| 通常フォルダのスタック集約一覧 | 実フォルダの親へ | 表示 / 表示 | stack は物理一覧の整理であり複数場所の集約ではない |
| fullscreen / detached viewer の読書面 | viewer の既存 close / parent | 描画しない | 本機能は一覧だけ。main 一覧が別窓と同時に表示される場合は main の判定で表示 |

ZIP / PDF を一律非表示にする必要はコード上ない。既存の「一段上」を矢印+Enterで辿れるため表示を推奨する。
検索・集約 owner まで拡張することも可能だが、root の終了と階層上昇を区別する表示仕様が増えるため初回案から外す。
この対象範囲は利用者判断事項であり、実装前に確定する。

## 5. B 案の描画・入力・選択仕様

### 5.1 表示位置の写像

`D = current_grid_order()`、`h = usize::from(parent_row_visible)`。
画面上の操作対象数は `D.len() + h`。h=1 なら位置0=Parent、それ以外は `D[p-h]`。
実項目 idx の画面位置は従来の表示位置+h。**raw idx 自体には足さない**。

- グリッドは先頭の **1マス** に上向きのフォルダアイコンと `..`。次の実項目は同じ行の次マス。
  一行全幅を予約せず、sticky にもしない。カテゴリの行割り当て・ソートから独立して絶対先頭に置く。
- 詳細表示は header の下に通常行高の1行。名前欄に `..`、他の metadata / check 欄は空。
  列幅・横スクロール・ヘッダソートの対象にはせず、専用 painter / hit rect で描く。
- `details_order` の長さを増やさず、name best-fit / tag prewarm / lazy meta / hover thumbnail は実項目だけ。
- `selected_cell_rect` は Parent の枠にも使えるが、実項目の選択情報を要求しない。親の tooltip で行先を案内する。
  下部選択情報は「親フォルダへ」等の短い案内または既存の空欄とし、直前画像の metadata を表示しない。

### 5.2 操作表

| 操作 | 提案する動作 |
| --- | --- |
| 修飾なし矢印 | 操作対象数 D.len()+h に既存 `grid_cursor_nav_target_pos` を適用。詳細は1列。OFF は既存と同じ |
| Parent から右 / 下 | 右は最初の実項目、下は表示位置 cols の実項目（末尾不足なら既存の clamp）。詳細は上下とも1行ずつ。空なら Parent に留まる |
| 実項目から先頭方向 | 既存 clamp / loop により Parent へ到達できる。ループ ON の上下は列を保つため、全ての末尾セルが Parent へ戻るわけではない |
| Home / End | Home は Parent（ある場合）、End は最後の実項目。親だけなら両方 Parent。既存 GridMoveFirst/Last を使用 |
| PageUp / PageDown | 同じ visible_rows × nav_cols の距離を表示位置へ適用し clamp。PageUp が0まで届けば Parent。親用の別ページ距離は作らない |
| Shift+矢印 | 従来どおりループなし。表示区間から Item だけを checkable 判定して追加し、Parent はチェックしない。Parent に着いても既存チェックは維持 |
| Enter (`GridOpenSelected`) | Parent は共通の親操作 helper、Item は既存 open。None は従来の未選択規則。Parent を最初の実項目に fallback して開かない |
| 左クリック | `..` は**修飾なし1クリックで親移動**を推奨。実項目の再クリックopen設定やダブルクリックpairingから独立したナビボタン扱い。入力許可・dialog / menu / right-drag抑止を共用 |
| Ctrl / Shift+クリック | Parent をカーソルとして選ぶだけ。チェック追加・範囲anchorにはしない。誤って親へ移動して複数選択を失わない |
| Space / Ctrl+A | Parent はチェック不能。Ctrl+A は従来の実項目だけ。checked 優先の一括操作はカーソルが Parent でも実チェックを対象にできる |
| 削除 / ★ / tag / pin / 外部ツール等 | 実チェックがなければ Parent は対象なし。選択対象の処理を現在フォルダや last image に fallback しない。現在コンテナを明示対象とする既存操作は維持 |
| 右クリック / ファイルD&D | Parent にファイル用メニュー・drag sourceは作らない。背景メニュー・ring / gestureを維持する場合も実 idx を渡さず、Parent rect を専用 hit として扱う |
| 背景クリック / Esc / 明示的な選択解除 | 現在の方式に従う。Explorer の余白解除は Parent も None にし anchor / checked を解除。Check 方式は維持。余白から親移動はしない |
| gamepad | direction は同じ投影を使用（詳細の左右ページ移動は既存仕様）、A は Parent なら親操作。B / ring の既存親操作は変更しない |

Parent を選ぶときは実クリックの範囲 anchor を外す。実項目クリック時は従来の apply_grid_click_selection に Item の index だけを渡し、
checked の動作を変更しない。Parent → Shift+実項目クリックは実 anchor 不在として現在の単項目選択規則へ委ねる。
キーボードで Parent へ動いたことだけを理由に checked を消さない。親移動成功後は既存ロードの選択/チェック規則を使う。
click pairing に偽の idx は登録しない。二つの独立した左クリックが各階層の `..` を押せば二段戻り得る（↑ボタンと同じ）。
これを抑えるための delay / timer は導入しない。一回の入力を click と double-click と Enter で二重実行しないことは handler test で保証する。

### 5.3 カーソル owner の通しと fallback

App と `ViewerContextBundle` の selected を同じ型で置換し、constructor / swap / mount / fork / snapshot / restore の全境界を揃える。
歴史的に実項目番号だけを保存する復帰 payload には `selected_item_index()` を投影する。
runtime の一覧をそのまま退避する snapshot は typed cursor を保持してよいが、復帰時の表示可否と items generation に照合する。
Parent をパス、ページ番号、book resume、永続 selected_file として保存しない。

`ensure_selected_visible_or_first` / `redirect_selected_to_visible` は有効な Parent を「実選択なし」として上書きしない。
設定 / owner 変更で Parent が表示できなくなったら先頭実項目へ、実項目も無ければ None。
通常フォルダ入場時の初期選択・読書位置・履歴復元は既存の**実項目**を優先する（毎回 Parent を自動選択しない）。
load / filterの選択確定境界で実項目0件かつ親表示可ならNone→Parent、親不可ならNoneとする。
カーソル None からの矢印開始は既存の先頭を基準に移動する規則を投影へ適用する。
背景解除の直後に描画の都合で自動再選択しない。filter / load の既存の選択確定境界だけで ensure を行う。

### 5.4 スクロール・サムネイルへの投影

総行数は grid=`ceil((D.len()+h)/cols)`、details=`D.len()+h`。
`apply_scroll_to_selected` は typed cursor の表示位置から行を求める。Parent は行0、選んだとき offset=0。
`snapped_scroll_extent`、最大offset、`pending_grid_scroll`、touch drag / pinch、scroll-only の先頭末尾は同じ増加後の extent を使う。
canonical offset は行高の整数倍、touch の端数は既存の一時描画状態に残す。

可視範囲 / keep-range / scroll_hint / visible_end_shared / details prewarm は**画面位置を実項目区間へ写像してから**使う。
strict visible の半開区間 `[a,b)` は `[clamp(a-h,0,D.len()), clamp(b-h,0,D.len()))` へ写像する
（減算は saturating）。prefetch の画面行区間も前後ページ分を広げてから同様に写像し、実 idx の疎な集合を維持する。
`update_keep_range_and_requests` 自身の offset→可視位置計算 (`src/app.rs:45762`) も対象であり、描画側の scroll_hint だけ直して完了しない。
Parent だけの strict visible 区間では visible priority の実項目要求を作らない。既存設定による次ページprefetchまで止めることはしない。
これはgrid由来の可視帯の話であり、navigation target / bookmark panel / still-seek / details hover等、
別の正当な利用元が所有する実ページ要求・保持投影をParent focusのために抑止しない。
現合成点は `src/app.rs:45700`, `:45869`、契約は `docs/display-pipeline.md` §1.8 を維持する。
実項目0件なら keep_set / worker boundary を空へする既存処理 (`src/app.rs:45721`) を維持する。
worker の idx / items_gen、thumb/meta 配列、キャッシュキー、メモリ会計は不変。
詳細の下部情報欄による gutter予測 (`src/ui_main.rs:19161`) も親込みの高さへ合わせ、バー有無でヘッダがずれないようにする。

## 6. 設定と再描画・keymap

- 永続名案: `Settings.grid_show_parent_nav_row: bool`、`#[serde(default)]` / `Default=false`。
  欠損キーの旧設定はOFF、独自DB table / migration / 初回ON切替は不要。
- UI 名案: **「一覧の先頭に親へ戻る『..』を表示する」**。環境設定 → **サムネイル** →
  「一覧のクリック選択」「カーソル移動をループする」の近くに「一覧の移動」枠として配置。
  両表示共通である説明と対象外の集約一覧を短く添える。新ページを作らない。
  現コード: `src/ui_dialogs/preferences/pages.rs:1506`, `:1532`; `preferences.rs:271`, `:3493`。
- 環境設定は draft を変更し、OK の `prepare_preferences_state_settings_for_commit` →
  `install_preferences_settings` (`src/ui_dialogs/preferences.rs:2448`) → `settings.save()` (`:2592`) で確定。
  Cancel / × の破棄では反映しない。既定リセットはOFF、再起動・再openで維持する。
- 変更の反映は通常の dialog 終了・次の一覧描画経路を使用。実 items の reload / sort / generation bump / thumb invalidation は不要。
  実カーソルとチェックを保持し、親が消えたときだけ§5.3の補正をする。
  設定ON確定時に実項目0件でNoneなら、親表示可の場合だけParentを選ぶ。背景解除後の毎描画でこの初期化を繰り返さない。
  列数・詳細切替と同じ選択行の ensure-visible / extent clamp を使用し、専用 live-rebuild worker は作らない。
  ON/OFF で実項目の行が変わる可能性は許容し、選択対象を画面内に保つ。これは設定をONにした場合の表示変更として説明する。
- parked context に settings のコピーや新しい「親表示更新待ち」は持たせない。
  next mount / 一覧描画前に global setting と現在 owner を照合して補正する。非表示の sibling の worker・世代は触らない。
- **新 KeyAction は不要**。Enter=`GridOpenSelected`、Home/End=`GridMoveFirst/Last`、Page=`GridPagePrev/Next`、
  親直行=`GridParentFolder` を再利用する。カスタムEnter割り当ても親カーソルで効く。
  `GridOpenSelectedAsPage/List` / external-player は実項目専用のまま、Parent では対象なし。
  修飾なし矢印・Shift範囲・通常マウス・Padは既存の固定入力層。
  `docs/keymap-spec.md` の grid 操作説明を更新し、Action / 既定 chord が変わらないので `keymap.ini.default` の Action は増やさない。
  IME / TextEdit / folder-pane focus / dialog / menu の input ownership と既存 keymap helper を迂回しない。

## 7. 親復帰・空フォルダ・読み込み中

- 実フォルダから親へ: `resolve_grid_parent_nav` が既存 `select_after_load` に元フォルダ名を設定し、
  `try_select_after_load` (`src/app.rs:63171`) が実項目を選ぶ。親の Parent カーソルは移動先へ持ち越さない。
  元フォルダが非表示なら既存の可視選択補正、消失なら既存の初期選択を使う。
- ドライブルートから戻る: `DriveList(Some(origin))` →既存ドライブ一覧入場で元ドライブを選ぶ。ドライブ一覧に Parent は無い。
- ZIP 内back: 既存 ZIP stack の選択/scroll 復帰を使う。ZIP/PDF/変換書庫から外へ出る場合は元コンテナの選択規則を使う。
- 空フォルダ / filterで実項目0件: 親移動可なら Parent 1マスだけ描画して Home / Enter / click を可能にする。
  現在の `items.is_empty()` (`src/ui_main.rs:18104`)、`visible_indices.is_empty()` (`:18200`) の両早期 return と
  `vi_len > 0` を、ナビ対象の有無で分ける必要がある。
  実件数表示は0のまま、空表示案内は残りの領域に出す。親不可なら従来の空表示。
- ロード中: 表示対象は**現在採用済みの一覧 owner**。request 先のパスから Parent を先出ししない。
  `grid_item_input_allowed` (`src/filename_stack_ui.rs:1878`) の location pending / stack refresh では Parent も項目入力を受けない。
  長いロードやmodalを回避する新しい例外入口は作らず、既存Backspace・バーで可能な取消・移動は維持する。
- 成功採用時に新 owner の表示可否とカーソルを確定。cancel / error / stale completion は既存 request owner の扱いに従う。
  Parent のための rollback保存や二重の選択復帰待ちは追加しない。失敗時は既存の通知を使う。

## 8. Remote・別ウィンドウ・スタック

**Remote:** PC 一覧の表示設定とし、Remote UI に新しい `..` entry は追加しない。
共有 scan / arrange / stack / page builder に Parent を入れないため、Remote の entry数・page index・resume・address・IPC version は変えない。
PC側設定のON/OFFで Remote の既存 parent / history / collection 動作を変えない。

**別ウィンドウ:** 表示するのは操作中の main 一覧。Parent cursor は main の bundle に属し、
表示・選択・設定切替からdetached / parkedのitems・selected・fullscreen_idx・channel・generation・読書位置を reset しない。
親移動を実行する場合は、既存親キーが対象にするowning sessionのclose / parkをそのまま維持する。
具体的にはZIP内部backの既存preserve/close (`src/app.rs:47844`) と、ZIP/PDFページ一覧からの親移動でactive detached sessionを終了する
`close_detached_viewer_for_virtual_page_list_parent_nav` (`src/app.rs:20527`, `:47857`) を省略しない。
`src/app/tests.rs:73969` は後者でpassive snapshotを作らず閉じる仕様を示す。全detachedを保持するようには変更しない。
不変を保証する相手は**当該親操作に無関係なsibling context**である。
別窓起動や既存窓前面化へ Parent を渡さない。実項目openに戻れば従来の detached reuse / always-newを使う。
typed selected を registry / ContextRef の実idx getter (`src/app/viewer_context_registry.rs:1492`) へ通す変更は、
ownership境界を保つ構造変更として実装前に設計 lead と独立 reviewer が確認する。
detached 述語 / viewport 経路を変更する場合は [detached-rework-plan.md §2](detached-rework-plan.md) の合意条件と§11への記録を満たす。
本書作成は合意済みの証拠ではなく、別窓の既存差異を症状パッチで塞ぐ許可でもない。

**スタック:** 通常フォルダの集約セルの前に描画し、代表画像・member / passthrough・count・flat mappingには入れない。
Parent Enter / click は `stack_try_open_from_grid` より前に振り分ける。
flat読書を閉じたときは既存の実stack/実画像選択を復元し、aggregateの先頭にParentを再投影する。
stack切替は元の実項目再選択を優先、カーソルがParentだった場合は同じ通常フォルダで表示可ならParentを保持する。
stack refresh の入力抑止・context / generation 照合は維持。サブ展開由来のstackには表のとおり出さない。

## 9. 実装・検証・文書更新計画（次段用）

### 9.1 実装のまとまりと所有

設計 lead が§10の判断を確定し、独立 reviewer が Parent owner、表示表、入力 routing、別context不変をレビューした後に実装する。
実装担当一名が shared App / ui_main / registry / preferences を所有し、同一ファイルの並行編集をしない。

1. cursorの単一型、実idx accessor、描画投影の純関数、親availabilityの基礎判定を導入。既存 selected のconsumer / producer / restoreを通す。
2. 既存親キーの実行を保持してhelper化、grid / details / 空一覧 / scroll / input / Padを接続。
3. 設定のdraft→OK→保存→再起動、snapshot、文書を通し、focused testから最終gateまで実装担当が一度ずつ実行する。

型移行単体を部分対応のfeatureとして出荷しない。全入口と復帰ownerが通った一まとまりを検収する。

### 9.2 自動テスト

| 層 | 必須ケース・期待値 |
| --- | --- |
| 投影・state純関数 | h=0/1、実項目0/1/複数、filterで0、detailsの逆順、列1/4/20、不完全最終行。raw idx・order・件数は変わらず、Parentを実idxへ変換できない |
| 表示可否 | §4.2全行。drive root、ZIP root/deep、PDF/EPUB/converted source、Ctrl+F origin/child、search root/drill、snapshot root/child、rating pending、collection root/child、sub展開、smart scope、通常stack |
| handler-level keyboard | AppTestEnv と fake egui key eventで先頭実項目→矢印→Parent→Enter。Home / End / Page、ループON/OFF・Shift区間、空一覧も確認。結果nav・select_after_load・実items/checked不変をassert |
| keymap・input ownership | GridOpenSelected=F13等の上書き、GridParentFolder上書き、NumpadEnter分離、TextEdit / IME / folder pane / dialog / menuでは親操作なし。同じ入力で二重navなし |
| pointer handler | 通常1クリック、Ctrl/Shiftは選択のみ、right-click・dragから実項目操作なし。Explorer / Check方式の余白解除、実anchor破棄、pairingの実idx汚染なし。実セルの既存ダブルクリック/reclickを回帰確認 |
| 項目操作 | ParentのみでSpace/削除/★/tag/pin/製本/外部ツールが対象を作らない。実チェックありなら従来のchecked優先。last imageへの誤fallbackなし。Ctrl+AにParentを数えない |
| scroll / layout | Parentを含む高さ、最大offsetの行snap、選択ensure-visible、Home=先頭、scroll-onlyでカーソル不変。列変更・details切替・狭い横幅・横バー/下部情報・touch端数。prewarm/keep-rangeに実idxだけ |
| 非同期 lifecycle | 親要求後の成功・空結果・拒否・取消・古い完了、ZIP内back。新Parent ownerは新一覧の採用時だけ決まる。設定変更でitems generation / worker request identityが変わらない |
| multi-context | Parentの表示/選択/設定切替でactive/parked detachedは不変。親実行時は既存のowning session close/parkをassertし、無関係なsiblingのitems/選択/scroll/世代/queueだけ不変。bundle mount/unmount / fork / restore、OFF中に戻したcontextのParent補正 |
| stack / Remote | aggregate/flat往復・設定切替でmember/order/count不変。Remote listing / resume / page数 / addressが設定ON/OFFで同一 |
| 設定の通し | Default/旧キー欠損OFF、serdeと使い捨てsettings.dbの保存読込、環境設定checkbox→OKで一覧に反映、Cancelで不変、reset OFF、再open/restart維持、favorite overlayで上書きされない |
| snapshot | 既存OFFのsnapshot維持、ONのgrid先頭・details先頭・親選択・空一覧・環境設定。light/dark、狭幅。PNGを目視確認 |

既存の基点: `src/app/tests.rs:16007` 以降のcursor-wrap、`:16234` Ctrl+F禁止、`:16271` drive-root、
`:16287` keymap override、`:16312` toolbar target、`:16328` Pad back、`:73969` virtual-page-list parent / detached。
preferencesの通しは `src/ui_dialogs/preferences.rs:5110` 周辺、snapshotは `tests/ui_snapshot.rs` のfixture方式を利用する。
純関数だけで「Enterが既存navへ到達する」を代替せず、handlerの結果をassertする。

実装後の検証順（本段では未実行）:

- `cargo check -p mimageviewer --bin mimageviewer-core`。
- `cargo test -p mimageviewer --lib parent_nav` 等の新テストfilterと既存 cursor / keymap / click / stack / collection / detached の焦点テスト。0件一致を成功にしない。
- 関連snapshot、`cargo fmt --check`、`python scripts/check_ui_glyphs.py`。
- selectedの共有境界に触れるため最終 `powershell -File .\scripts\test-full.ps1`。
- gate通過後 `.\scripts\build-dev.ps1` で通常profileの利用者確認用coreを準備（この設計段では不要）。

build/checkは10分以上、full testは15分以上の実行枠を確保する。
実アプリの検証は次段でも自動で開始せず、シナリオ・所要時間・desktop/input使用・使い捨てデータを提示した上で明示了承を得た別枠にする。
利用者確認には「通常フォルダを矢印+Enterで往復、drive root、ZIP deep/PDF、空、details、OFFへ戻す、別窓を開いたmain一覧」を含める。

### 9.3 実装時の文書・マニュアル更新対象

この段では本書以外を変更しない。次段で更新する場所:

- `docs/spec.md`: 設定名・既定値・対象一覧・件数/選択からの除外。
- `docs/keymap-spec.md`: gridの矢印/Home/Page/Enter、Parent時のSpace・項目操作、クリック・Pad。`keymap.ini.default` はAction不変の生成一致を確認。
- `docs/virtual-folders.md`: ZIP内一段/外へ戻る、PDF/EPUB/元書庫のParent表示がページ数に含まれないこと。
- `docs/filename-stack-plan.md`: aggregateの先頭枠とflat/サブ展開の対象外境界。
- `docs/top-level-grid-view.md` / `architecture-overview.md`: cursor ownerと復元の型契約が変わる範囲。module/worker新設は不要。
- `docs/README.md`: 本設計への索引。detached経路を触れた場合のみ `docs/detached-rework-plan.md` §11へ合意と範囲を記録。
- `htdocs/mimageviewer/manual/settings.html`、`shortcuts.html`、`tut-navigation.html`、`tut-grid-power.html`: 「サムネイル」設定、対象場所、1クリックとEnter、空フォルダ/ドライブ/本内の説明。
- `docs/next-release-backlog.md` §1.297: 実装・検証・利用者確認の状態を反映。公開時の変更履歴はrelease leadへ引き継ぐ。§1.298は保留のまま。

## 10. 利用者判断事項（推奨付き）

| 判断 | 推奨 | 他案と費用 |
| --- | --- | --- |
| 方式 | **B: データ外の先頭1マス + typed cursor** | Aは全item機能へ疑似kind対応。全幅行はgridの占有と例外ナビが増える |
| ZIP/PDF/EPUB等のページ一覧 | **通常由来は表示。ZIPは内部階層のbackを優先** | 非表示なら仕様は狭いが、本を含む往復にBackspaceが必要。追加entryをページ数へ混ぜない費用はBで吸収できる |
| ドライブルート | **表示してドライブ一覧へ。tooltipで行先明示** | 非表示は「物理親だけ」に揃うが、矢印+Enterだけで別driveへ移れない |
| 検索・★固定・rating・collection・履歴・sub展開・smart | **owner内全体で非表示** | 子だけ/由来一覧への復帰も表示する案は便利だが、「上へ」「一覧終了」「元結果へ」の意味分類・tooltip・復帰テストが増える。既存Backspace/↑は保持 |
| マウス操作 | **修飾なし1クリックで親移動**（要件の「クリック」に合わせる） | 実項目と同じ1クリック選択+ダブルクリックopenなら誤移動は減るが手数増。二段連続クリックを防ぐ待ち時間は採用しない |
| 初期カーソル | **既存の実項目/履歴復元を優先。空ならParent** | 毎回Parentを選ぶと、Enterで先頭画像/フォルダを開く現在の流れが変わる |
| 設定名・配置・共有範囲 | **§6の名前、環境設定「サムネイル」、PC一覧のみ、既定OFF** | 表示モード別 / 場所別 / Remote別の複数設定は組み合わせと説明を増やす |

いずれもこの設計段では提案。実装開始前にdesign leadが選択を記録する。
通常操作を不便にする変更が別途必要だと分かった場合は、既存機能を削る案を勝手に採用せず費用とともに相談する。

## 11. 本段の確認記録

2026-10-04、別contextの `gpt-6.1-sol` / `xhigh` に草案と関連ソースのread-only限定レビューを依頼。
既存親移動によるowning session終了とsibling不変の区別、表示位置写像の計算量、独立サムネ利用元の保持、
空一覧のParent初期選択境界を本文へ反映した。限定確認範囲で実装を妨げる設計矛盾は残らないとの報告を受けた。
これは§10の利用者選択や、detached経路変更への設計lead合意を代替しない。

本書のソース参照パス・行番号範囲、文書リンク、末尾空白を機械確認した。
作業前のtreeはclean、作業後は本書だけがuntracked。HEADは `ceec8d5d2` のまま。
製品コード変更・Cargoテスト・ビルド・製品バイナリ起動・コミットは実施していない。
