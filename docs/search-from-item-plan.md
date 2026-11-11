# 名前で検索 (§1.690) — 設計

作成: 2026-11-11。ブランチ `feat-690-search-from-item` (master `3287af3fc` から)。設計担当 ClaudeCode Opus。
状態: **最小版の設計 (第 2.4 版)。仕様は利用者承認済み (2026-11-11、統括経由)。第 2 版の独立レビュー (Sol/xhigh) は REJECT
(P1 1 件・P2 5 件・P3 1 件)。第 2.1 版で全件を反映 (各所に「P<N> 対応」)。第 2.1 版の再レビューも REJECT (P1 2 件・P2 1 件・P3 1 件)。第 2.2 版で反映。第 2.2 版の再レビューは ACCEPT WITH CHANGES (P2 1 件: 全選択の印の期間)。第 2.3 版で印の期間を縮め、
実装で判明した Ctrl+G の再投影の退行に対して手順 3 を「ふだんの編集と同じ後始末」に改めた。第 2.3 版も実装で成立しなかったため、
第 2.4 版で「Ctrl+G・Everything の検索セッションが開いている間はアイテム検索へ渡さない」に絞った (M3 手順 3)。実装・指定の自動検証済み。確認用ビルドの native 依存失敗は M9 に記録。**
第 1 版 (材料を組み立てる専用のモーダル) は利用者決定で後の版へ回した。付録 A に残す (未レビュー)。

## M1. 範囲 (利用者決定 2026-11-11)

次の版は**最小版**にする。項目の名前を、既存の検索欄へ「入れるだけ」にする。

1. 入口: メイン窓の一覧の右クリックに「名前で検索」のサブメニュー。中身は
   「Everything で探す」「アイテム検索で探す」「この場所で絞り込む」の 3 つ。
   同じ 3 操作を `KeyAction` に足す (既定キーなし、操作カスタマイズで割り当て可)。
2. 選ぶと、その渡し先の検索欄を開き、入力欄を項目の名前で置き換える。名前全体を選択した状態でフォーカスし、
   **検索はまだ実行しない**。利用者が編集して Enter で検索、Esc で取りやめ (既存の検索欄の動きそのまま)。
   渡し先の入力欄に前の文字があれば置き換える。
3. 文字は加工しない (引用符で囲まない・文字を削らない)。空白を含む名前は、各検索のふだんの解釈 (AND など) になる。
4. 対象はメイン窓の一覧で右クリックした 1 項目 (キー操作ならカーソルの項目)。別ウィンドウ (F12)・フルスクリーン・
   チェックした複数項目・一覧の背景は対象外 (メニューに出さない / キーは何もしない)。
5. §1.756 の入力履歴には、Enter で検索した時点でふだんどおり入る。名前を入れただけでは入らない。
6. 「渡し先 + 文字列」の小さな型を持たせる (§1.692 の土台)。作り込まない。

後の版へ回したもの: 名前の部分・親フォルダ名・タグ・AI のモデル名を選んで組み立てる画面、タグビューへの受け渡し、
文字どおり / 検索式の切り替え、コンテナ検索 (Ctrl+S) への受け渡し、まとめ (Stack) の対象化。

## M2. 入れる名前

| 項目の種類 | 入れる文字 |
| --- | --- |
| 画像・動画・音声 | ファイル名から拡張子を除いたもの (`Path::file_stem`) |
| フォルダ | フォルダ名そのまま (`.` を含んでも削らない) |
| ZIP・PDF・EPUB・変換書庫 (ファイルの本) | 拡張子を除いたもの |
| ZIP 内の画像 | 書庫内の basename から拡張子を除いたもの |
| ZIP 内のフォルダ (ZipDir) | そのフォルダ名 |
| Everything・アイテム検索・コンテナ検索の結果行 | 実体の種類 (上の各行) と同じ。`SearchContainer` は実体がフォルダならフォルダの規則、ファイルなら拡張子なし |
| 対象外 (メニューに出さない) | PDF のページ (「Page N」は名前ではない)、まとめ (Stack)、コレクションの欠落行、背景 |

- 拡張子を除くのは最後の `.` 以降だけ (`a.tar.gz` → `a.tar`)。`.` で始まり他に `.` が無い名前 (`.hidden`) はそのまま。
- 名前が空になる場合はメニューを無効にする (実際にはほぼ起きない)。
- 名前は表示名ではなく実体の名前から作る (一覧の表示名の設定で変わらない)。

## M3. 渡し先ごとの動き

既存の検索欄の開き方をそのまま使う。新しい開き方を作らない。

| 渡し先 | 開き方 | 選べない条件 (メニューを灰色にし、理由を hover に出す) |
| --- | --- | --- |
| Everything で探す | `ensure_search_session(Everything)` (常設欄が無ければ一時バー) | `everything_search_entry_availability` が使えない |
| アイテム検索で探す | `ensure_search_session(Metadata)` | `pinned_search_availability(Metadata)` が Err |
| この場所で絞り込む | `ensure_search_session(Local)` | `pinned_search_availability(Local)` が Err (本のページ表示中など) |

**受付の条件 (第 2.2 版。2.1 版のレビュー指摘 P2-5・P2-6、再レビュー指摘 P1-2 対応)**: 各渡し先は、**その検索のふだんのショートカット
(Ctrl+R / Ctrl+G / Ctrl+F) が今受け付けられるときだけ**選べる。ただし「検索を開始できる状態か」(渡し先ごとの判定) と、
「キー入力を誰が所有しているか」(入力所有・IME の判定) は分ける。ショートカット側の受付・入力ガードは変えない。

- 渡し先の判定は、ショートカットの dispatch が使っている「検索を開始できる状態か」の条件を 1 つの述語に取り出して共有する。
- **右クリックメニュー自身もモーダル扱い** (`common_modal_dialog_open` に含まれる) なので、メニューの灰色判定では
  **呼び出し元の右クリックメニューだけを除外**した述語を使う (他のモーダル・閲覧中などはそのまま拒否)。
  Everything の dispatcher の keyboard permit はメニューへ流用しない。
- メニューで選んだ後は、対象の名前を文字列に確定し、**自分のメニューを閉じて (context_menu の owner を片付けて) から**
  `open_search_with_text` を呼ぶ。`open_search_with_text` はメニューを除外しない通常の述語で受け付ける。
  egui のメニューとネイティブメニューの両方の dispatch 経路で同じ順にする。
- キーは既存 `handle_keyboard` の入力所有・IME・detached 前面・フォルダペイン判定の後で処理する (コピー系の `KeyAction` と同じ位置)。
  その後 `open_search_with_text` の通常の述語。
- 「この場所で絞り込む」は、アイテム検索・Everything の結果を表示している間 (結果から開いた実フォルダ
  `PhysicalChild` を含む) は灰色。ふだんの Ctrl+F がこの間は受け付けられないのと同じ (`update_phases.rs` の Ctrl+F 受付)。
  検索を終了して戻り先を復元する経路にはしない。
- 入口そのもの (メニュー項目とキー) は、メイン窓 (main viewport、フルスクリーンでない) のサムネイル一覧で、
  `grid_item_input_allowed()` が真、ブックマーク一覧・閲覧履歴の一覧でない、チェックした項目が無い、
  M2 の対象種別の 1 項目、のときだけ有効。メニュー・キー・dispatch でこの判定を 1 つの関数にまとめて共有する。

共通の手順 (`App::open_search_with_text(ctx, request)`、新設):

1. 受付の条件を確認。満たさなければ理由を既存の通知で出して何もしない
   (メニューでは灰色にしているので、届くのはキー操作のときだけ)。
2. `ensure_search_session(kind)`。開いていなければ既存の開く関数 (別の検索セッションとの排他・戻り先の保持を含む) が走る。
   既に開いていれば何もしない。Ctrl+G の `toggle_global_search` (開いていれば閉じる) は使わない。
3. その欄の下書き (`search_draft_mut(kind)`) を名前で置き換える。**検索を始める処理 (Edited / Submit) は呼ばない。**
   Ctrl+G (第 2.4 版。第 2.2 版・第 2.3 版の手順は実装で成立しなかったので取りやめた):
   今の Ctrl+G は、検索語の下書きと表示中の結果・prepare の再投影・適用条件が結び付いていて
   (`SearchViewWish.query` が今の下書きから作られる、編集で結果を消して取り直す)、**開いている Ctrl+G / Everything の検索セッションの
   下書きを外から差し替える組み合わせ**は、旧い検索の取り消し・再投影・Everything の採用済み結果のどれを選んでも食い違いが出た
   (第 2 版・第 2.1 版のレビュー指摘 P1、第 2.2 版の prepare 退役で並べ替えが名前順に退行、第 2.3 版の reset で
   prepare が収束しない・Everything の旧い語が再公開される)。CLAUDE.md の「組み合わせを減らす」に従い、この組み合わせを作らない:
   - **Ctrl+G・Everything のどちらの検索セッションも開いていないときだけ**「アイテム検索で探す」を選べる。
     開いている間 (結果の表示中・結果から開いたフォルダ・入力中) は灰色にし、理由「検索を閉じてから使えます」を出す。
     「この場所で絞り込む」がこの間すでに灰色なのと同じ扱い。
   - 選べるときは、**下書きを名前にしてから** `ensure_search_session(Metadata)` (= `open_global_search`) を呼ぶ。
     `open_global_search` の既存の「保持された下書きは編集・確定まで待機」の分岐 (`done = true`、`last_change_at` なし) にそのまま乗り、
     reset もデバウンスも呼ばない。常設欄に前の語が残っていても置き換わるだけで、既存の常設欄の再表示と同じ状態になる。
   - 後の版で、検索中にも Ctrl+G へ渡したい要望が出たら、下書きと結果の結び付きを分ける構造変更 (結果の組み立てを実行済みの語から作る) として扱う。
   Ctrl+F と Everything は確定 (Enter / 検索ボタン) でしか検索しないので、代入だけでよい。
4. フォーカスの要求を**毎回明示的に立てる** (P2-2 対応。既に開いている検索では開く関数が走らないため):
   常設欄なら `request_pinned_search_focus(ctx, kind)`、一時バーなら Local は `search_focus_request = true`、
   Metadata / Everything は `global_search.focus_request = true` (`temporary_search_focus_reservation` が拾う形)。
5. 全選択 (第 2.2 版、P2-3・再レビュー指摘 P2 対応): egui はフォーカスの無い欄の選択を単一カーソルに畳むので、
   描画前に保存した選択は残らない。Everything の欄 (常設・一時バーとも) は既存の「フォーカス要求の pass で全選択」がそのまま効くので、
   **印を作らない**。Local と Metadata だけ、対象の入力欄の id (常設欄なら `kind.input_id()`、一時バーなら `kind.temporary_id()`)
   について、egui の一時データに印 (発行した pass 番号 `ctx.cumulative_pass_nr()` と入れた文字列) を置く
   (`crate::search_request` の小さな helper で置く・消費する)。egui の一時データは明示的に消すまで残るので、
   **対象の入力欄が描かれた最初の pass (発行 pass 以降) で必ず取り出して消す**。その pass がフォーカス要求の pass か欄にフォーカスがあり、
   IME 変換中でなく、下書きが印の文字列と同じで、発行した pass かその次の pass (第 2.3 版。3 pass から縮めた) なら、Everything の欄と同じ方法
   (show の後で `state.cursor` に全範囲を入れて store し、`output.cursor_range` も更新) で全選択する。それ以外は消すだけ。
   呼ぶ場所は、常設欄の描画 (`ui_pinned_search.rs` の 1 か所) と、Ctrl+F・Ctrl+G の一時バーの描画。ふだんの Ctrl+F・Ctrl+G の開き方は変えない。
6. 何も実行しない。確定は利用者の Enter (既存の `submit_search_input`)、取りやめは既存の Esc。

- 渡し先の他の選択 (Ctrl+F・Ctrl+G の OR、検索対象、Ctrl+G の絞り込み) は変えない。
- §1.756 との関係: 履歴は `submit_search_input` の中で記録されるので、Enter したときにふだんどおり入る。
  名前を入れただけでは記録されない。§1.756 の「入力欄から離れたときの記録」は、今の語がその欄で実行済みの語と
  同じかどうかで決まる。したがって**実行済みの語と違う名前を入れて、Enter せずに離れた場合は記録されない**。
  入れた名前が直前に実行済みの語と同じだった場合は、既存の離脱処理どおり、記録・先頭への移動が起こる (履歴を消去した後なら再登録。仕様どおり) (P3-7・再レビュー指摘 P3 対応)。
  §1.756 の描画部品 (`ui_history_field`) は触らない。`ui_pinned_search.rs` へは手順 5 の 1 行だけ足す
  (§1.756 も同じファイルを変えているので、後から入る側が取り込んで合わせる)。
- §1.682+§1.688 とはファイルが重ならない。

## M4. 型 (§1.692 の土台)

```rust
// src/search_request.rs (新設、純粋)
pub(crate) struct SearchRequest {
    pub target: crate::ui_pinned_search::PinnedSearchKind, // 初版は Local / Metadata / Everything
    pub query: String,
}
pub(crate) fn item_search_name(item: &GridItem) -> Option<String>; // M2 の規則
```

- 「入れるだけ」(今回) と「入れて実行する」(§1.692 で必要になれば) の区別は呼び出す関数で分け、型には持たせない。
- 永続データは増えない (保存しない)。スキーマ・設定の変更なし。

## M5. 入口

- 右クリック: `ContextMenuItemId` にサブメニュー 1 つ (「名前で検索」) と葉 3 つ、`MenuCommand` に 3 つ。
  `ALL` の末尾に追加、stable_name は `SearchByName` / `SearchByNameEverything` / `SearchByNameMetadata` / `SearchByNameLocal`。
  単一項目の分岐で、M3 の入口の条件を満たすときだけ出す。位置はコピー系の近く。
  **既存の親分類は `Root` と `OpenWith` だけ**で、未知のサブメニューは `OpenWithSubmenu` 扱い、プレビューの再帰も子を
  `OpenWith` 扱いにしている (`context_menu_model.rs`)。そこで親分類 `SearchByName` を足し、サブメニュー自身の配置 ID、
  子の layout 適用、プレビューの親解決、網羅検査 (サブメニュー自身を含む) まで既存の `OpenWith` と同じ形でつなぐ (P2-4 対応)。
  ネイティブメニューの木構造の描画と、灰色項目の理由表示は既存の処理を使う。
- キー: `KeyAction::GridSearchNameEverything` / `GridSearchNameMetadata` / `GridSearchNameLocal`、context Grid、
  既定 `ChordList::EMPTY`。`ini_name` / `context` / `trigger` / `ALL_ACTIONS` / 表示名 / `docs/keymap.ini.default` /
  `docs/keymap-spec.md` をそろえる。対象は `self.selected`。メイン窓の一覧にフォーカスがあるときだけ。
- 右クリックとキーは同じ関数 (`open_search_with_text`) を呼ぶ。

## M6. 状態と寿命

- App に新しい状態は持たない。持つのは M3 手順 5 の全選択の印だけで、これは入力欄 id に結び付いた egui の一時データ
  (Local・Metadata だけ。発行 pass と文字列を持ち、対象欄の最初の描画で必ず消費する。全選択の条件は M3 手順 5)。
- 名前はメニュー・キーの時点で文字列にする。以後の一覧の入れ替えとは無関係。
- 検索欄を開いた後の操作 (編集・Enter・Esc・×・別の検索への切り替え) はすべて既存の検索欄の処理。
- 検索欄が使えない状況 (閲覧中・モーダル表示中) は `pinned_search_availability` が既存どおり判定する。

## M7. テスト

- 単体: M2 の名前 (各種別、`a.tar.gz`、`.hidden`、拡張子なし、フォルダ名の `.`、ZIP 内の階層付き entry、対象外)。
- headless App: 3 つの渡し先 × (常設欄 ON / OFF) で、セッションが開き、下書きが名前に置き換わり、フォーカスと全選択がそろい、
  検索が始まっていないこと (Ctrl+F は `search_pending`・`search_filter` 不変、Ctrl+G は pending なし・検索中の表示なし、
  Everything は producer なし)。その後の Enter で既存どおり検索が始まること。
  別の検索セッション (Ctrl+S から各渡し先、Ctrl+G から Everything) が開いているときの切り替え。
  Everything から Ctrl+G を含め、利用できない渡し先では何も変わらないこと。
- Ctrl+G が「検索完了後」「デバウンス待ち (`last_change_at` あり)」「検索の実行中」のそれぞれで、
  名前の注入を拒否し、下書き・結果・検索の所有状態が変わらないこと (第 2.4 版、P1)。
- 既に開いていて入力欄からフォーカスが外れている Local / Everything (常設・一時バーとも) に入れても、
  フォーカスが入り全選択されること。開設済み Metadata は第 2.4 版の受付で拒否する (P2-2)。
- 全選択の印: Local・Metadata の常設欄と一時バーで、フォーカスが入った描画で全選択になること。ふだんの Ctrl+F・Ctrl+G で
  開いたときは今までどおり全選択しないこと。IME 変換中は選択を変えないこと (P2-3)。
- 受付: アイテム検索・Everything の結果表示中 (`PhysicalChild` を含む) は「この場所で絞り込む」が灰色で、キーでも何も起きないこと。
  ブックマーク一覧・閲覧履歴の一覧・フルスクリーン・detached viewer・チェックありでは入口が出ない / 効かないこと (P2-5・P2-6)。
- §1.756 取り込み後: 入れただけ・違う語で離れた場合は記録されず、Enter で記録されること。
- (第 2.4 版) Ctrl+G・Everything の検索セッションが開いている間 (結果表示中・結果から開いたフォルダ・入力中・常設欄) は
  「アイテム検索で探す」が灰色で、キーでも何も起きないこと。開いていないときは、常設 ON/OFF とも、下書きが名前になり、
  検索中の表示にならず、poll を回しても検索が始まらず、prepare が待機に収束し、Enter で検索が始まること。
- (第 2.3 版) 全選択の印を置いた直後に欄が描かれないまま終了し、2 pass 以上後にふだんの Ctrl+F/G で開き直しても全選択されないこと。
  印を置いた pass か次の pass で描かれた場合は全選択されること。
- (第 2.2 版) 右クリックメニュー表示中に 3 項目が (条件を満たすとき) 有効で、選ぶとメニューが閉じて欄が開くこと
  (egui メニューとネイティブメニューの dispatch の両方)。他のモーダルが開いているときは灰色。
- (第 2.2 版) 全選択の印を置いた後、欄が描かれない pass が続く・Esc で閉じる・source を切り替える、のどれの後でも、
  ふだんの Ctrl+F・Ctrl+G で開き直したときに全選択されないこと。Everything には印が作られないこと。
- 右クリックメニュー: 対象種別だけに出る・複数選択や背景では出ない・無効の理由。キー操作: 既定未割り当て、割り当てれば動く。

## M8. 文書

- マニュアル (検索のページと一覧の右クリック・キー一覧)。`docs/search-architecture.md` (§1.5 の後に入口を一言)、
  `docs/item-kind-capability-matrix.md` (§3 に行)、`docs/keymap-spec.md`、`docs/README.md` の索引。
- 通信・保存の約束は変わらない。

---

## M9. 実装記録 (2026-11-12)

### 第 2.1 版での停止記録

### 設計担当への確認事項

M3 の「検索のふだんのショートカットと同じ受付条件」を右クリックメニューの表示・実行にも
そのまま適用すると、メニュー自身により全渡し先が拒否される。コード調査と独立レビューで一致した。
ユーザー指示「設計の主要な前提がコードと合わなければ、実装で迂回せず止めて報告する」に従い、
この境界を変更する前に実装を停止した。以下の差分は未完成・未検証であり、取り込み可能な完成状態ではない。

- `src/app.rs` の `modal_dialog_block_reason_including_menus` は
  `include_menus && self.context_menu_idx.is_some()` を `grid_context_menu` として扱う。
  `any_dialog_open` / `common_modal_dialog_open` がこの判定を参照する。
- `src/ui_dialogs/context_menu.rs` の `show_context_menu` は `context_menu_idx` を保持したまま
  `context_menu_nodes` を生成し、選択コマンドを dispatch した後に消す。
  `dispatch_native_context_menu_result` もメニュー owner を保持したまま dispatch する契約である。
- このため Ctrl+F/G の既存 `any_dialog_open`、Everything の既存 entry availability、
  さらに `pinned_search_availability` が、すべてメニュー表示中の要求を拒否する。
  Everything には pass ごとにキャッシュした `keyboard_owner_for_pass` の `Modal` も関係するため、
  `context_menu_idx` の一時解除だけでは解決しない。
- 必要な設計判断: メニュー自身が入力を所有する間の可否評価と、メニューを閉じて検索欄へ渡す実行境界を
  どこで区切り、通常ショートカットの受付を変更せず共通条件を共有するか。独自 guard・一時解除・
  owner キャッシュの書換え等の迂回は追加していない。

### 作成した差分

- 新設: `src/search_request.rs` (要求型・名前生成・全選択の印)、
  `src/app/search_request.rs` (受付の共通化・下書き注入・キー入口)、
  `src/app/tests/search_request.rs` (headless 回帰テストの草稿)。
- 接続: `src/lib.rs`、`src/app.rs`、`src/app/update_phases.rs`、
  `src/context_menu_model.rs`、`src/ui_dialogs/context_menu.rs`、`src/keymap.rs`、
  `src/ui_main.rs`、`src/ui_pinned_search.rs`、`src/global_search_ui.rs`、
  `src/global_search_ui/everything/shortcut.rs`、`src/global_search_ui/everything/ui.rs`。
- 文書: `docs/keymap.ini.default`、`docs/keymap-spec.md`、`docs/key-customization-impl-plan.md`、
  `docs/search-architecture.md`、`docs/item-kind-capability-matrix.md`、`docs/README.md`、
  `docs/architecture-overview.md`、本書、`htdocs/mimageviewer/index.html`、
  `htdocs/mimageviewer/manual/search.html`・`grid.html`・`shortcuts.html`。
- テスト追加: 名前・全選択 helper 6 件、keymap 1 件、メニューモデル 3 件、
  App 9 件 (常設 ON/OFF・3渡し先・再注入・通常開き方・Ctrl+G 3時点・受付・切替・キー)。
  いずれもコンパイル・実行前。M7 全体の充足は未確認。

### 実行した検証と未実行項目

- `python scripts/check_site_cards.py`: 53 カード、違反 0 件 (文書担当・実装担当)。
- 公開 HTML 4 ページ内のローカル fragment 参照 65 件の検査: 問題 0 件 (文書担当)。
- `git diff --check`: 成功。改行コードに関する Git の警告あり。
- `cargo fmt` (引数なし、ワークスペース全体): 成功 (exit 0)。
- `python scripts/check_ui_glyphs.py`: 危険なグリフ 0 件 (exit 0)。
- `cargo check`、関連 `cargo nextest run`、`cargo test --test ui_snapshot`、
  検証バイナリ作成: 設計判断待ちで未実行。
  本体 lib 全件テスト・製品バイナリ起動・git 書き込み操作は行っていない。

### 第 2.2 版への対応と追加の設計確認事項

利用者の続行指示を受け、設計コミット `b04a445b1` と途中実装コミット `d440d8827` の上で作業を再開した。
第 2.1 版のメニュー受付問題は、以下の変更で対応した。git の書き込み操作と製品バイナリ起動は行っていない。

- `src/app/search_request.rs` と `src/app/update_phases.rs`: 検索開始可能な状態の判定を共通化し、
  Everything の keyboard permit は `src/global_search_ui/everything/shortcut.rs` の dispatcher へ戻した。
- `src/app.rs`、`src/app/pinned_search.rs`: 通常の受付を維持したまま、メニュー自身だけを除いた可否判定を用意。
  `src/ui_dialogs/context_menu.rs` は名前確定 → 自分の owner を閉じる → 通常の `open_search_with_text` の順とした。
- `src/global_search_ui/everything.rs`: `retire_search_work` を source 切り替えと Metadata 注入で共有。
  Metadata 注入は旧 pending / prepare / mtime lookup を退役し、run を更新、done=true・reject解除・debounce解除とする。
  accepted と last_executed、表示中の items は注入時には変更しない。
- `src/search_request.rs` と `src/ui_pinned_search.rs`・`src/ui_main.rs`・`src/global_search_ui.rs`:
  Local/Metadata の印を発行 pass + 文字列とし、最初の描画で消費する。全選択は同語・非IME・focus要求またはfocusあり・3pass以内。
  `src/global_search_ui/everything/ui.rs` の印消費を削除し、Everything は既存の全選択だけを使う。
  通常の `open_search_shortcut` では Local/Metadata の両入力IDの印を明示破棄する。
  これにより、3pass以内の Esc / source切り替え後にも通常の開き直しで注入の印が復活しない。
- `src/app/tests/search_request.rs`・`src/ui_dialogs/context_menu.rs` の headless 回帰を更新・追加。
  `docs/search-architecture.md` の内部説明、`docs/keymap.ini.default` の生成順、本書 M6 の古い寿命説明も合わせた。

独立レビューで、**第 2.2 版の prepare 退役により保持結果の評価順ソートが退行する**ことが判明した。
コード経路でも一致したため、利用者の「設計の前提がまたコードと合わなければその時点で報告」に従い、
この境界の追加実装を停止した。現在の差分は取り込み可能な完成状態ではない。

1. Metadata 注入は、検索完了後を含め `retire_search_work` で `page_edit_prepare` を消す。
2. 検索結果を表示したまま評価順へ切り替えると、`App::apply_sort_change_reload_with_physical_mode`
   (`src/app.rs`) は `rebuild_items_from_global_search` を呼ぶ。
3. `request_search_view_build` (`src/global_search_ui.rs`) は prepare の有無で分岐する。
   prepare が無い Metadata の経路では、`ListingOrderRequest::Rating` を `standard_fallback()` に変え、
   「評価順を準備できなかったため名前順で表示しました」と通知する。評価順のまま ★ フィルタを変えた場合も同じ再構築へ進む。
4. Metadata の保持結果の並べ替えには prepare を作り直す入口が無い。
   Everything の dataset 再投影経路とは異なる。第 2.2 版の指定どおり旧 prepare を消すだけでは、
   名前注入前に使えていた並べ替えを保持できない。

設計担当への判断事項: 旧検索・旧 query の prepare を退役させた後、保持結果をどの immutable 条件・結果から再投影するか。
旧 prepare をそのまま残す、評価順を無効化する、名前順への fallback を容認する、等の迂回・機能制限は追加していない。

第 2.2 版対応時の検証:

- `cargo check -p mimageviewer --bin mimageviewer-core`: 成功 (8分07秒)。既存コードの warning あり。
- `python scripts/check_ui_glyphs.py`: 危険なグリフ 0 件。
- `python scripts/check_site_cards.py`: 53 カード、違反 0 件。
- `cargo nextest run -p mimageviewer --lib -E 'test(/search_request|search_name|search_by_name/)'`:
  依存 `turbojpeg-sys v1.2.0` の native build で失敗 (CMake の MSBuild install が exit 1、Cargo exit 101)。
  テスト実行 0 件。追加テストのコンパイル完了も未確認。タイムアウトによる打ち切りではない。
- `cargo fmt` (引数なし): 成功 (exit 0)。
- `git diff --check`: 成功。改行コードについての Git の警告のみ。
- 関連モジュールの nextest、`cargo test --test ui_snapshot`、検証バイナリ作成: 追加の設計判断待ちで未実行。
  `test-lib-full.ps1` は利用者指定により実行しない。

---

### 第 2.3 版への対応と追加の設計確認事項

利用者の続行指示を受け、`5e1e42553` の上で以下を変更した。

- `src/app/search_request.rs`: Metadata 注入後に既存の `reset_global_search_for_query_change(ctx)` を呼び、
  直後に debounce 解除・done=true・reject解除とする。
- `src/global_search_ui/everything.rs`: 不要となった `retire_search_work` を削除し、`switch_search_source` の後始末を元の形へ戻した。
- `src/search_request.rs`: 印の期間を発行 pass / 次 pass のみに縮め、境界テストを更新。
  テスト内の `CharIndex` / 整数比較も修正した。
- `src/ui_dialogs/context_menu.rs`: テストが private な `SubmittedEverything` を直接作る代わりに、
  既存の fake dataset 採用 fixture を使う。trait import と `PhysicalChild` fixture も修正。
- `src/app/tests/search_request.rs`: 旧結果を実際に採用した fixture から 3 時点の注入を検査し、
  並べ替え（名前・評価昇順・評価降順）× ★ 変更を含める。既存の settle helper の可視性だけを
  `src/global_search_ui.rs` の test module 内で広げて再利用した。
- `docs/search-architecture.md` と `htdocs/mimageviewer/manual/search.html`: 第 2.3 版の説明へ更新。

独立レビューと source inspection により、既存 reset を debounce 解除だけで未実行状態へ使う場合に
以下の 2 経路の問題が指摘された。2 の Everything の旧語再公開は headless テストでも捕捉した。
1 も既存の settle helper を使う headless 検査で 30 秒以内に待機へ収束せず失敗した。
利用者指示に従って、この所有境界の追加実装を停止した。取り込み可能な完成状態ではない。

1. **通常フォルダ → Metadata 注入**: reset により `last_executed` は空になる。
   `poll_search_page_edit_prepare` の Ready 採用条件は、Metadata では
   `items_are_global_search_view || !last_executed.is_empty()` を要求する。
   通常一覧では両方 false のため採用できない。items generation が不変で worker が正常 Ready を返し続ける場合は、
   desired が残って再送され、100ms 後の再描画要求が続く。世代変更・worker の Failed / 切断では待機へ抜ける。
   最初の headless 検査では 3 秒内に 3 回の stale rejection は観測せず成功したが、
   これは待機への収束自体を証明しないため、既存の settle helper を使う検査へ改めた。
   再検査は `Metadata projection did not settle: query=new draft done=true rejected=None applied=` で失敗した。
   既存検索結果からの 3 時点の検査では `items_are_global_search_view=true` なので、この入口を検査できない。
2. **Everything 結果 Root → Metadata 注入**: source 切り替えと reset は accepted の Everything dataset を保持する。
   `request_search_view_build` の先頭は `everything_dataset_for_reprojection` を見て旧 dataset を再投影する。
   この判定は編集 source を見ておらず、Everything の Ready は accepted の submitted と一致すれば採用される。
   そのため旧 Everything の結果・`last_executed` が再公開され、空の Metadata 未実行状態にならない。

設計担当への確認事項: 後続の検索実行が無い場合について、通常一覧からの未送信入力と、
別 source の採用済み結果を消して待機する処理をどの所有境界で成立させるか。
Ready の採用条件を緩める guard や、accepted を呼び出し側で書き換える迂回は追加していない。

この 2 経路の期待する不変条件を headless 回帰に追加した:

- `search_name_new_metadata_draft_converges_without_repeated_projection_rejections`
- `search_name_metadata_from_everything_does_not_republish_the_old_query`

コンパイル・検証の途中記録:

- 可視性・CharIndex の報告済みエラーを修正。最初のコンパイルで追加検出された test fixture の
  borrow / `NodeT` import も修正した。
- 最初の追加テスト: コンパイル成功 (16分02秒)、35 件中 31 成功・4 失敗。
  Everything の旧語再公開以外の 3 件は fixture の問題で、未設定カーソルを許す検査、
  child viewport の RawInput map、IndexerInit の terminal 状態を修正した。
- 利用者の PC 負荷規則追加前に開始した関連テスト・ui_snapshot は中断され、完了結果として扱っていない。
  以後の全 cargo は `CARGO_BUILD_JOBS=6`、nextest は `-j 6` を付け、重いコマンドは 1 本ずつ実行する。
- 制限適用後の関連 nextest (修正前): コンパイル 8分53秒、実行 111.401 秒。
  776 件中 773 成功・3 失敗。2 件は上記の Metadata 注入の不一致。
  残る `context_menu_musicvideo_saved_order_and_hidden_preserve_existing_items` は、非表示を slot 配分より先に適用し、
  他項目が外部ツール / Open With の固定枠を跨ぐ不具合だった。
- `src/context_menu_model.rs` を、順序解決 → ID の表示設定・子 layout 解決 → 空 unit 除去 → 区切り線出力の順へ修正。
  既存期待値は維持し、leaf / submenu の非表示と、空 submenu の Present / Absent 指定の漏れの回帰を追加した。
  独立レビューでこの配置修正は ACCEPT。Metadata 注入の未解決 2 件は承認対象に含まない。
- `docs/section221-context-menu-layout.md`、`docs/context-menu-unification-plan.md` へ同じ解決順を追記。
- メニュー配置の最初の絞り込み検査は 144 件すべて成功 (追加 P2 対応前の source での結果)。
  追加 P2 対応後の関連全体と ui_snapshot の結果は以下へ追記する。

git 書き込み・製品バイナリ起動・`test-lib-full.ps1` は行っていない。

#### 第 2.3 版対応時の最終検証結果

以下の cargo コマンドはすべて `CARGO_BUILD_JOBS=6` と `MSBUILDDISABLENODEREUSE=1` を設定して 1 本ずつ実行した。
nextest は `-j 6`、ui_snapshot は `RUST_TEST_THREADS=6` も設定した。

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功、28.84 秒。既存の warning あり |
| `cargo nextest run -p mimageviewer --lib -j 6 --no-fail-fast -E 'test(/context_menu_model\|ui_dialogs::context_menu\|keymap\|pinned_search\|global_search_ui\|everything\|search_request/)'` | 最終差分で 778 件中 776 成功・2 失敗、実行 84.460 秒（コンパイル 3分17秒） |
| `cargo test --test ui_snapshot` | 249 件すべて成功、実行 61.50 秒（コンパイル 2分51秒）。PNG 更新なし |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ 0 件 |
| `python scripts/check_site_cards.py` | 成功、53 カード・違反 0 件 |
| `cargo fmt`（引数なし）と `git diff --check` | ともに成功。Git の改行コード警告のみ |

残る nextest の失敗は、上記の未送信 Metadata の不一致を検出する次の 2 件だけである。

- `search_name_metadata_from_everything_does_not_republish_the_old_query`:
  `last_executed` に `"old everything"` が再公開されることを検出した。
- `search_name_new_metadata_draft_converges_without_repeated_projection_rejections`:
  prepare が待機へ収束せず、既存 settle helper が 30 秒で失敗した。
  `query=new draft done=true rejected=None applied=`。

3 時点からの注入・旧検索取消・結果消去・並べ替え（評価順を含む）/ ★ 変更・Enter、
入力欄の focus / 全選択と印の寿命、メニュー・キーの入口、keymap 往復、配置・preview の各回帰は
上記 2 件を除いて関連検査で成功した。設定済みメニュー順の回帰も最終差分では成功した。

最終ログは `target/search-from-item-verification/related-final-j6.log`、`ui-snapshot-j6.log`、`core-check-j6.log`。
検証用バイナリは、未解決の設計不一致により関連テストが失敗しているため作成していない。
第 2.3 版の実装は未完了であり、失敗する 2 件を無効化したり、製品へ迂回処理を入れたりしていない。

今回の変更ファイル（`5e1e42553` 以降、12 件）:

- `src/app/search_request.rs`
- `src/app/tests/search_request.rs`
- `src/context_menu_model.rs`
- `src/global_search_ui.rs`
- `src/global_search_ui/everything.rs`
- `src/search_request.rs`
- `src/ui_dialogs/context_menu.rs`
- `docs/context-menu-unification-plan.md`
- `docs/search-architecture.md`
- `docs/search-from-item-plan.md`
- `docs/section221-context-menu-layout.md`
- `htdocs/mimageviewer/manual/search.html`

---

### 第 2.4 版への対応

設計 `023e18461` と途中実装 `bcfc446b9` の上で、開設済み Metadata / Everything から Metadata への
注入を拒否する仕様へ変更した。第 2.3 版の失敗を迂回する処理は追加していない。

- `src/app/search_request.rs`: 注入専用の共通受付で `global_search.active` 中の Metadata を
  「検索を閉じてから使えます」で拒否する。通常ショートカットの受付は変えない。
  許可時は Metadata の下書き代入 → `ensure_search_session(Metadata)` とし、既存の保持された下書きの待機分岐を使う。
  第 2.3 版の reset 呼び出し・debounce / done / reject の直接変更を削除した。
- `src/app/tests/search_request.rs`: Metadata の完了後・デバウンス待ち・実行中、および Everything からの注入は
  拒否・状態不変・割り当てたキーでも不変の検査へ置換。常設 ON/OFF で、未開設時の注入後に poll しても
  prepare / producer を作らず、元の一覧を保持して待機し、実際の Enter で初めて検索を始めることを検査する。
  開設済み欄への再注入の成功検査は Local / Everything に限定した。
- `src/global_search_ui.rs`: 不要になったテスト用 settle helper の公開範囲を元に戻した。検索処理の挙動は変更していない。
- `src/ui_dialogs/context_menu.rs`: 成功時の通常一覧保持と、両 source × 入力中 / 結果 / 子フォルダ / 実行中 × 常設 ON/OFF の
  灰色理由・既存 owner 不変を検査。ネイティブ dispatch と実際の headless egui ボタンの無効状態・クリックの両方を含む。
- `docs/keymap-spec.md`、`docs/search-architecture.md`、`docs/item-kind-capability-matrix.md` と本書:
  第 2.4 版の受付・待機処理に更新。M7 の旧い再注入条件も新しい拒否条件と区別した。
- `htdocs/mimageviewer/manual/search.html`・`shortcuts.html`: 開設中の制限を記載し、検索説明を短縮。
  `htdocs/mimageviewer/index.html` の検索カードは `3287af3fc` の文へ完全に戻した。

独立レビューでは production と追加回帰に実装 finding なし。文書指摘の M7 の旧条件は上記のとおり整理した。
第 2.4 版の検証結果は以下に記録する。

初回の関連 nextest はコンパイル成功（7分35秒）、780 件中 777 成功・3 失敗（実行 100.293 秒）。
失敗はテストの前提設定・期待値だったため、製品コードを変えずに修正した。

- Everything の拒否テストは、結果用 prepare が元から存在するため、`None` ではなく拒否前後の有無・同じ cancel Arc・非取消を比較する。
- 2 件のメニューテストは、保持下書きで開いた `done=true` のまま擬似ストリームを送っていたため、poll が受信前に戻っていた。
  結果の前提状態を通常の検索開始と同じ `reset_for_new_query` で初期化し、既存 helper の実行語 `test` に下書きも合わせた。
  App の 3 時点の拒否テストも、実際に採用された spec を比較し、後から適用条件を上書きしない前提設定へそろえた。
  これらの修正にも独立レビューの追加指摘なし。

#### 第 2.4 版の最終検証

本体のコンパイルを伴うコマンドはすべて次の共通スクリプトを経由し、1 本ずつ実行した。
ロックへの書き込み不可の警告が出たため、利用者指示どおり低優先度・6 並列のフォールバックで継続した。
`CARGO_BUILD_JOBS=6`、`MSBUILDDISABLENODEREUSE=1`、nextest は `-j 6`、ui_snapshot は `RUST_TEST_THREADS=6` も設定した。
各時間は Cargo / nextest の表示値で、共通ロックの待機時間を含まない。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "codex-690" -WorkDir G:\home\mimageviewer-690 -Command "<コマンド>"
```

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功、43.08 秒。既存の warning あり |
| `cargo nextest run -p mimageviewer --lib -j 6 --no-fail-fast -E 'test(/context_menu_model\|ui_dialogs::context_menu\|keymap\|pinned_search\|global_search_ui\|everything\|search_request/)'` | 修正後は 780 件すべて成功、実行 62.373 秒（コンパイル 3分03秒）、16540 件は範囲外 |
| `cargo test --test ui_snapshot` | 249 件すべて成功、実行 60.64 秒（コンパイル 1分07秒）。PNG 更新なし |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ 0 件 |
| `python scripts/check_site_cards.py` | 成功、53 カード・違反 0 件 |
| `cargo fmt`（引数なし）・`cargo fmt --check` | 成功 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 失敗、exit 101。`turbojpeg-sys v1.2.0` の CMake / MSBuild install が exit 1。core 未生成 |

ログ: `target/search-from-item-verification/core-check-24.log`、`related-24-final.log`、`ui-snapshot-24.log`。
初回の失敗記録は `related-24.log` に残している。

確認用ビルドは、共通ロック待機と既存の native ビルド待機を経て実行した。
`[vcrt-pe] passed: runtime=4 pe=0` の後、未準備だった dev-runtime 用依存のコンパイルに入り、
`target/dev-runtime/build/turbojpeg-sys-ad9ab6b47bbc75e3/out/build` の CMake install で失敗した。
`MSBUILDDISABLENODEREUSE=1` は最初から適用済み。`mimageviewer-core.exe` は生成されず、後続 companion のビルドにも進んでいない。
既存の debug / test 用 native 依存を使う指定の自動検証はすべて成功しているが、
利用者向けの確認用バイナリは native 依存の install 失敗を解消した環境で再作成する必要がある。
製品コードや依存ソースを迂回する変更・ビルドキャッシュの手動差し替えは行っていない。

第 2.4 版からの実装の逸脱、独立レビューの未解決指摘はない。
変更は上記の 11 ファイル。整形は引数なしの `cargo fmt` でワークスペース全体へ適用した。
git 書き込み・製品バイナリ起動・本体 lib 全件テストは行っていない。実機 UI / IME の確認も未実施。

---

# 付録 A: 後の版の拡張案 (設計第 1 版、2026-11-11、未レビュー・利用者未提示)

以下は最小版の前に書いた案。利用者決定で後の版へ回した。後の版で着手するときは、最小版の型 (M4) を土台に見直す。
後の版の候補として、コンテナ検索 (Ctrl+S) への受け渡しと、まとめ (Stack) の対象化も加える。

## 1. 要求と出どころ

- バックログ §1.690 (`docs/next-release-backlog.md`) の「採択した案 (利用者 2026-11-10)」が仕様の土台。
  5ch >>704・>>718 の要望 (同じ作者・シリーズを探すため、名前のコピー → 検索欄へ貼り付け → 編集、の手数を減らしたい)。
- 利用者の方針: 項目のタグやファイル名などの情報を、ほかの検索へ渡せる汎用的な作りにする。
- 自動推測 (作者名・シリーズ名の推定)、勝手な文字の削除、送った直後の自動実行はしない。

## 2. 利用者から見た仕様

### 2.1 開き方

- メイン窓のサムネイル一覧で項目を右クリック →「この項目で検索…」。小さなモーダルを開く。
- キー操作 `GridSearchFromItem` (一覧のカーソル位置の項目)。**既定は未割り当て** (`GridCopyFileName` と同じ扱い)。
  既定キーを付けるかは利用者に確認する (§9)。
- 初版の対象はカーソルのある (右クリックした) 1 項目だけ。チェックした複数項目・一覧の背景・別ウィンドウ
  (F12 の detached viewer)・フルスクリーンからは開かない。チェック中に右クリックしたときの複数項目メニューにも出さない。

### 2.2 対象の項目と材料

| 項目の種類 | 名前 (拡張子あり / なし) | 区切りで分けた部分 | 親フォルダ名 | タグ | AI のモデル名・生成ツール |
| --- | --- | --- | --- | --- | --- |
| 画像・動画・音声 | ファイル名 | 拡張子なしの名前から | 親フォルダ | 付いていれば | 読めれば (画像) |
| フォルダ | フォルダ名 (拡張子なしは出さない) | フォルダ名から | 親フォルダ | 付いていれば | なし |
| ZIP・PDF・EPUB・変換書庫 (本) | ファイル名 | 拡張子なしの名前から | 親フォルダ | 付いていれば | なし |
| ZIP 内の画像 | 書庫内のファイル名 (basename) | 同左 | 書庫の名前 (拡張子なし) | なし (種別表どおり一覧ではタグ不可) | 読めれば |
| PDF のページ | なし (「Page N」は名前ではない) | なし | PDF の名前 (拡張子なし) | なし | なし |
| 出さない | まとめ (Stack)・ZIP 内フォルダ (ZipDir)・コレクションの欠落行・背景 | | | | |

- Everything / アイテム検索 / コンテナ検索の結果行は、通常の種類 (Image・Folder 等) と同じ扱い。
- 「区切り」は次の文字: 空白 (全角空白を含む)、`_`、`-`、`.`、`,`、`[ ]`、`( )`、`{ }`、`【 】`、`「 」`、`『 』`、
  `（ ）`、`［ ］`、`～`、`~`。`・` は名前の中で使われるので区切りにしない。空の部分は捨て、同じ部分は 1 回だけ、
  名前の中の順に並べる。数字だけの部分も残す (推測で落とさない)。
- タグは `tags_cache` (無ければ `hydrate_tags_cache_for_paths` の 1 件読み、既存のタグ操作と同じ扱い) から表示名で並べる。
- AI 情報は情報パネルと同じ `metadata_cache` を引く。無ければ worker で読み込みを始め、届くまで「AI 情報を読み込み中」と出す。
  読めない・無い場合は行を出さない (まれな失敗は割り切り: ログのみ)。UI スレッドでファイルを読まない。
- 「名前の一部を選んで足す」: 名前全体を読み取り専用の 1 行入力欄に出し、文字を選んで「選んだ部分を足す」で足せる。

### 2.3 画面の構成 (上から)

```
┌ この項目で検索 ─────────────────────────────── × ┐
│ 名前: [ [作者A] シリーズB 第03話.zip          ] [選んだ部分を足す] │
│ ┌ 材料 (クリックで下の入力欄に足す) ───────────────┐ │
│ │ 名前      [[作者A] シリーズB 第03話.zip] [[作者A] シリーズB 第03話] │
│ │ 区切り    [作者A] [シリーズB] [第03話]                     │
│ │ フォルダ  [漫画]                                         │
│ │ タグ      [作者A] [連載中]                                │
│ │ AI        [モデル名] [生成ツール]                         │
│ └──────────────────────(ここだけスクロール)┘ │
│ 検索する語: [ 作者A "シリーズB"                  ] [×]  │
│ 足すとき:  (●) 文字どおりに探す  ( ) 検索式として渡す        │
│ 渡し先:    (●) Everything (Ctrl+R)  ( ) アイテム検索 (Ctrl+G) │
│            ( ) 現在地の絞り込み (Ctrl+F)  ( ) タグビュー        │
│            (選べない渡し先は灰色にして理由を横に出す)           │
│                                  [検索]  [取りやめ]          │
└──────────────────────────────────────────────┘
```

- 材料はボタンを折り返して並べる。材料の部分だけ縦スクロールし、名前・入力欄・渡し先・ボタンはスクロールの外に置く
  (1093×614 でも「検索」「取りやめ」・×が見えること)。
- 「検索する語」の入力欄は空で始まる (自動推測しない)。キーボードで自由に編集できる。日本語入力は `crate::ime_focus` の helper。
- 開いたときのフォーカスは入力欄。Enter (IME 確定ではない) で「検索」、Esc で取りやめ。

### 2.4 入力欄と「文字どおり / 検索式」

**入力欄の文字列が、そのまま渡し先の検索欄に入る** (見たとおりのものが渡る)。
材料をクリックしたときに、どう足すかを「足すとき」で選ぶ。

- 文字どおりに探す (既定): 空白や記号を含む語を引用符 `"…"` でくくってから、空白 1 つで区切って末尾に足す。
- 検索式として渡す: 材料をそのまま足す。利用者が手で書いた演算子 (`|`、`!`、`-`、`ext:` など) はどちらの場合もそのまま渡る。

引用符でくくる条件 (渡し先によらず 1 つの規則。Everything と mIV の構文の両方で安全な側に倒す):

- 空白 (全角空白を含む) を含む、`-` で始まる、`| ! < > : * ?` のどれかを含む。
- くくっても意味は変わらない: mIV の構文 (Ctrl+F・Ctrl+G) は `"…"` を 1 語のフレーズとして扱い、Everything は
  引用符の中を演算子も含めて文字として扱う (voidtools の Search Syntax、2026-11-11 に参照)。
- `"` を含む語は、どちらの構文でも文字どおりにくくれない (mIV の構文には引用符のエスケープが無く、Everything の
  `&quot:` は 1.5 だけ)。Windows のファイル名には `"` が入らないので、起き得るのはタグ・AI 情報だけ。
  その語はそのまま足し、入力欄の下に「引用符 (") を含む語は文字どおりには渡せません。必要なら書き換えてください」と出す。
  勝手に文字を消さない。

渡し先ごとの補足:

- Ctrl+G は既存の最小文字数の規則で 1 文字の語などを拒否する。拒否の表示は Ctrl+G の検索欄の既存の表示に任せる。
- Ctrl+F・Ctrl+G の OR トグル・検索対象などの選択は、渡し先の今の状態のまま変えない (置き換えるのは検索語だけ)。

### 2.5 渡し先

| 渡し先 | 選べる条件 (開いた時点の状態で判定) | 「検索」で起きること |
| --- | --- | --- |
| Everything (Ctrl+R) | Everything 検索が使える (`everything_search_entry_availability`) | Everything の検索欄の語を置き換えて送信 |
| アイテム検索 (Ctrl+G) | 常に | アイテム検索の語を置き換えて検索開始 |
| 現在地の絞り込み (Ctrl+F) | 本のページ表示中ではない (`pinned_search_availability` と同じ理由) | 絞り込みの語を置き換えて実行 |
| タグビュー | 入力欄の語 (前後の空白と、外側の引用符 1 組を除く) が、この項目に付いたタグの 1 つと一致する | そのタグでタグビューを開く (`open_tag_view_for_tag`) |

- 選べない渡し先は灰色にして、理由を 1 行で出す。
- 渡し先の初期値は、この起動中に前回選んだもの (保存しない)。初回は Everything が使えれば Everything、だめならアイテム検索。
- 入力欄が空白だけのとき「検索」は押せない。

## 3. 検索要求の型 (§1.692 が載る土台)

新しい純粋モジュール `src/search_request.rs`:

```rust
/// 文字列で検索する渡し先。既存の検索欄の種類をそのまま使う。
pub(crate) enum SearchRequestTarget {
    Field(crate::ui_pinned_search::PinnedSearchKind), // Local / Metadata / Everything (Container は §9)
    TagView,
}
pub(crate) struct SearchRequest {
    pub target: SearchRequestTarget,
    pub query: String, // 渡し先の検索欄にそのまま入る文字列
}
```

- 同じモジュールに、材料の分割 (`split_name_parts`)、引用の要否と付与 (`quote_literal_term`)、入力欄への追記
  (`append_term`)、タグビュー判定 (`tag_for_tag_view`) を純粋関数として置き、単体テストする。
- §1.692 (保存した検索) はこの型を保存・呼び出す形で載せる。OR トグル等の渡し先ごとの選択はこの型に入れない
  (初版の範囲外。§1.692 で必要になったら足す)。保存はしないので、永続データ・スキーマの変更は無い。

`App::dispatch_search_request(ctx, request)` (新 `src/app/search_request.rs`) が唯一の実行口:

1. 呼ぶ前にモーダルの状態を片付けておく (モーダルが開いたままだと `pinned_search_availability` が Err を返すため)。
2. `Field(kind)`: `pinned_search_availability(kind)` を確認 → `ensure_search_session(kind)` →
   `*search_draft_mut(kind) = query` → `submit_search_input(ctx, kind, kind.input_id())`。
   常設欄の「検索」ボタンと同じ確定の経路なので、Ctrl+F の実行・Ctrl+G の検索開始・Everything の送信・検索欄の状態は既存のまま。
   利用できなければ、既存の通知 (`show_feedback_result`) で理由を出して何もしない。
3. `TagView`: `open_tag_view_for_tag(tag)`。

## 4. 他のブランチとの関係

- **§1.756 (検索欄の入力履歴、`feat-756-search-history`)**: 履歴は `submit_search_input` が受け付けた時点で
  その欄の下書きを記録する。この機能は同じ関数を通すので、**渡した語は「確定した語」として履歴に入る**
  (Ctrl+R/G/F。設定がオフなら入らない)。タグビューは §1.756 でも記録しない。この機能は履歴のコードに触れない。
  §1.756 が `finish_search_input_submit` を `finish_text_input_submit` に改名しているが、この機能は呼ばない。
- **§1.682+§1.688 (検索中の場所ジャンプ、`feat-682-search-nav`)**: 触るファイルが重ならない
  (この機能は `ui_main.rs` の場所▼・アドレス欄に触れない)。
- この機能が触る共有ファイルは、右クリックメニューのモデル (`context_menu_model.rs`)、その dispatch
  (`ui_dialogs/context_menu.rs`)、`keymap.rs`、`app.rs` (状態 1 欄・描画呼び出し・`common_modal_dialog_open`)。
  検索欄の描画部品 (`ui_pinned_search.rs`、`app/pinned_search.rs`) は変更しない。
- 先に master へ入った側を取り込んで合わせる。

## 5. 状態と寿命 (組み合わせを減らす)

- **モーダルにする**: 開いている間は一覧・検索・閲覧の操作を受け付けない (`common_modal_dialog_open` に登録)。
  渡し先の状態や一覧が裏で変わる組み合わせを扱わない。
- **材料は開いた時点で文字列に写し取る** (index を持ち続けない)。後から届くのは AI 情報だけで、パスから作った
  キーで照合する (一覧が入れ替わっても別の項目の情報が入らない)。
- 渡し先の選べる/選べないも開いた時点で判定する。押した時点で使えなくなっていたら (まれ)、通知を出して何もしない。
- 「検索」はモーダルを閉じてから 1 回だけ実行する。取りやめ・×・Esc は何もせずに閉じる。
- 状態は `App` の `Option<SearchFromItemDialog>` 1 つ。無ければ閉じている。
- 採用しなかった案: 材料をクリックした順の「語の並び」を内部に持ち、文字どおり/検索式を切り替えたら作り直す方式。
  入力欄を自由に編集すると並びと文字列が食い違い、同期の組み合わせが増えるため。

## 6. キー割り当て

- `KeyAction::GridSearchFromItem`、context は Grid、ini 名 `GridSearchFromItem`、表示名「この項目で検索する」、
  既定 `ChordList::EMPTY`。`ALL_ACTIONS`・`trigger()`・`docs/keymap.ini.default` (コメント行)・`docs/keymap-spec.md` をそろえる。
- 対象はグリッドのカーソル位置 (`self.selected`)。右クリックの dispatch と同じ開く関数を呼ぶ。

## 7. 右クリックメニュー

- `MenuCommand::SearchFromItem` / `ContextMenuItemId::SearchFromItem` (stable_name `SearchFromItem`、
  `ALL` の末尾に追加)、表示「この項目で検索…」。単一項目の分岐で、メイン窓のグリッド (`surface == Grid`) かつ
  §2.2 の対象種別のときだけ出す。位置はコピー系 (名前をコピー等) の近く。利用者のメニュー編集・ネイティブメニューは既存の仕組みに乗る。
- `ContextMenuPreviewScenario` の網羅テストを満たす。

## 8. テスト

- 単体 (`search_request.rs`): 区切り分割 (全角括弧・連続区切り・重複・数字だけの部分・空)、引用の要否 (空白・全角空白・
  先頭 `-`・`|!<>:*?`・`"` を含む語)、追記の空白、タグビュー判定 (外側の引用符、前後空白、一致しない)。
- 材料の作成: 種別ごと (§2.2 の表) に名前・親・タグの有無。ZIP 内画像の親が書庫名、PDF ページに名前が無いこと。
- dispatch (headless App): 3 つの渡し先それぞれで、下書きが置き換わり、その検索のセッションが開き、既存の確定経路
  (Ctrl+F は `search_pending`、Ctrl+G は run 番号の更新、Everything は producer) が動くこと。利用不可の渡し先で
  何も変わらないこと。タグビューが開くこと。
- モーダル: Enter で検索・Esc で取りやめ・IME 変換中の Enter で確定しない・開いている間はグリッドのキーが効かない・
  Enter が下のグリッドへ漏れない。
- 右クリックメニューの項目 (対象種別だけに出る) とキー操作 (既定未割り当て、割り当てれば開く)。
- 1093×614 と UI 倍率 200% で「検索」「取りやめ」・× が見えること。UI スナップショットを 1 枚追加。

## 9. 利用者に確認すること

1. 入力欄 = 渡す文字列そのもの、「文字どおり / 検索式」は**材料を足すときの形**を選ぶもの、という解釈でよいか (§2.4)。
2. 既定のキー割り当てを付けるか (案: 付けない。付けるなら候補は空いているキーから提示)。
3. 渡し先にコンテナ検索 (Ctrl+S) を足すか。同じ経路で足せる (親フォルダ名で同じシリーズのフォルダ・書庫を探す用途)。
4. 対象外にした まとめ (Stack) を初版で対象にするか (まとめの名前を材料にできる)。

## 10. 文書

- マニュアル: 検索のページと一覧の右クリックの説明に「この項目で検索」。キー一覧。
- 設計文書: `docs/search-architecture.md` (§1.5 の後に入口を追記)、`docs/item-kind-capability-matrix.md` (§3 に行を追加)、
  `docs/keymap-spec.md`、`docs/README.md` の索引。
- 通信・保存の約束は変わらない (保存しない、外部へ送らない。Everything へは既存の検索と同じ IPC)。

## 11. まれな失敗

まれな失敗は割り切り (ログ + 通知)。AI 情報の読み込み失敗は行を出さないだけ。再試行の仕組みは作らない。
