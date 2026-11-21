# §1.835 絞り込みの変更で一覧を読み直さない — 自動更新の判定 key から facet を外し、AI の絞り込みを一覧全体の取得の証明へ移す (設計)

- 状態: 設計 r5 (2026-11-21)。r3 のレビューの P2 3 件・P3 1 件 (§4.2.2 の 5・7・8、§4.2.4、§7) と、r4 のレビューの P2 1 件 (§4.2.2 の 9) を反映した。
  - r1・r2 の独立レビュー (Sol xhigh) は、どちらも REVISE。構造的修正であることには、どちらも AGREE。
  - r2 の指摘 (AI の取得の対象と、完了の資格) を受けて、利用者が案 A を選んだ (統括経由、「AI も直して v5.5.1」)。
  - r3 では、AI の絞り込みを、長さと同じ「一覧全体の走査 + 取得の証明」に乗せる (§4.2)。
  - 収録版は v5.6.0 (2026-11-21 利用者決定「ツリーだけ先に出す」で v5.5.1 から変更)。
- 書いた人: 設計担当 ClaudeCode。
- 根拠: コードと、サブ PC 担当の観測ログ (`C:\home\mimageviewer-subpc\duration-762\results\`、リポジトリ外)。
- 行番号: master `ebfa00b91` の時点。

## 1. 観測と原因

- 観測 (サブ PC 担当、2026-11-21):
  - 長さの範囲指定で数字を 1 文字打つと、その値で絞り込まれてメニューが閉じた。
  - 2 文字目以降は一覧に届き、Enter で選択中の動画が開いた。
  - 絞り込みや並べ替えを変えるたびに、ログに `auto-refresh: folder content changed (…), reloading` が出た (17 回)。
  - ファイルは変わっていない (SHA-256 で照合)。
- 経路 (コード):
  1. 絞り込みを変えると、`settings.facet_filter` が変わる。
  2. 監視の点検 (`src/app.rs:27774-27788`) は、`current_folder_projection_key` が監視作成時の key と違うと、監視を作り直して `start_external_rescan` を呼ぶ。
  3. key を作る `folder_projection_key_with_presentation` (`src/app.rs:5076`) の材料に、`"facet": settings.facet_filter` がある。
  4. `start_external_rescan` は、baseline を同じ key の観測に限る (`src/app.rs:27985-27993`)。baseline が無いので結果は「一致」にならず、`auto-refresh … reloading` から全体を読み直す。
  5. 読み直しの sidecar restore 中は、モーダル扱い (`modal_dialog_block_reason_filtered` の `"sidecar_restore"`) になる。`note_main_chrome_pointer_block_transition` が `egui::Popup::close_all` を呼んでメニューを閉じ、キーを保留する。
- 長さとサイズの範囲指定は、チェック + DragValue でできている。DragValue は編集中も 1 文字ごとに値を更新するので、1 文字目でメニューが閉じる。
- ほかの絞り込みも、変えるたびにフォルダ全体を読み直している。1 回選んで閉じる項目なので目立たない。
- 所有:
  - key の行は `a73ea3750` (2026-10-14、§1.375 R10) から。
  - サイズの範囲指定は、v5.4.0 でも同じ経路を通る (コードからの推論、実機未確認)。

## 2. 守るべき不変条件

自動更新の観測 (`FolderObservation`) は、次の 2 つを対で持つ。

- 走査・materialize が作った物理行の記録 (`FolderRowRecord`)
- その行を作った投影条件

key は「この行の記録を作った条件」を識別するためのもの。条件が変われば、旧記録を比較に使えない。

- §1.375 の記録の契約 (`docs/section375-operation-reconcile-plan.md` §3.1・§8.1) では、行の記録の境界は「種類採否・同名 winner・補助画像・既存 rating sort と表示 block 順まで」。
- facet と details は、「items 配列ではなく既存 visible_indices / details_order の投影を保持する」(同 §8.1)。
- つまり facet は行の記録の外にあり、行を作る条件ではない。

facet の判定に要るデータの取得 (長さ・AI メタデータなど) は、facet の変更や候補の集合の変化に依らず、要件の持ち主が要件と完了の証明で決める (§4.2)。

## 3. 違反を作っている箇所

key に facet が入っているので、行を作る条件が同じでも key が変わる。そのため facet の変更が「行を作る条件の変更」として扱われ、ファイルの変更が無いのに全体の読み直しが走る。

- 行を作る側は facet を読まない。`src/app/book_materialization.rs`・`src/app/folder_scan.rs`・`src/grid_item.rs`・`src/rating_sort.rs` に `facet` の参照は 0 件。
- facet は、`rebuild_visible_indices` → `passes_facet_filter` (`src/app.rs:75759`・`:76668`) だけで効く。
- 同じく表示だけの絞り込みである評価フィルタ (`settings.rating_filter`) は、元から key に無い。
- facet の中でも、`name_query` と `place_keys` は `#[serde(skip)]` のため、元から key に入っていない。
  - これは key に無いという事実を述べているだけで、それで正しく動くことの実証ではない。
- §1.375 計画 §3.3 は「sort／facet変更で旧要求を拒否する」と書いているが、facet を入れた理由は書かれていない。行の記録の契約 (§2) とは食い違っている。

読み直しが、AI の絞り込みの取得不足を隠していた (§4.2.1)。

## 4. 修正

修正は 2 つ。

- `folder_projection_key_with_presentation` の材料から `"facet"` を外す。外した理由をコメントに書く。
  - 理由: facet は、行の記録の後の表示投影 (visible_indices) で、`rebuild_visible_indices` が今の設定で作り直す。行の記録の key に入れると、ファイルが変わっていないのに facet の変更で全体を読み直す。
- AI モデル・ツールの絞り込みの取得を、長さと同じ「一覧全体の走査 + 取得の証明」へ移す (§4.2)。
  - key から facet を外すと表に出る既存の欠陥で、同時に直さないと退行になる。

key に残すもの:

- `sort`
- `order` (grid_display_order = 表示 block 順)
- `visibility` (種類採否)
- 同名本・skip 系・`autofs`・`books`
- stack 関係
  - 生の `FolderRowRecord` の条件ではないが、記録から決定的に作る group 表示の条件で、保持される group 表示に要る (`src/app/filename_stack_ui.rs:187`・`:1319`)。
  - したがって key は「行の記録と、そこから決定的に作る group の条件」を識別する。

`sort` から上の項目は、いずれも行の記録の境界の内側。

key を使う箇所は、全部同じ関数を通る。関数を変えれば、全箇所で一律に意味が変わる。呼び出し側は変えない。

- 監視の作り直し: `src/app.rs:27774`
- 再走査の baseline: `src/app.rs:27973`
- worker の一致判定: `src/app.rs:28101`
- 結果を捨てるかの判定 `proof_valid`: `src/app.rs:28219`
- 一致の採用: `src/app.rs:28342`
- 観測の作成: `src/app.rs:28349`・`:32560` ほか
- テスト用の補助 (`#[cfg(test)]`、製品経路ではない): `src/app.rs:28408`・`:28440`
- detached の fork: `src/app/viewer_context_registry.rs:5159`

### 4.1 facet を変えた後に受理される旧要求

外すと、facet の変更前に始まった再走査の結果を、変更後に受理することになる (`proof_valid`)。

- 一致 (Equal) の場合: items をそのまま保つ。visible_indices は、facet の変更時に今の設定で作り直してある。
- 不一致 (Different) の場合: `MaterializedFolderListing` を通常の load で install し、visible_indices は今の facet で作り直す (`src/app.rs:44985`)。
  - `MaterializedFolderListing` は、古い facet や visible_indices を持たない (`folder_scan.rs:897`、r1 レビューで照合)。

### 4.2 AI の絞り込みの取得を「一覧全体の走査 + 取得の証明」へ移す (r3)

#### 4.2.1 今の欠陥 (r1・r2 レビュー、ClaudeCode がコードで確認)

AI モデル・ツールの絞り込み (以下 AI facet) は、取得の範囲と完了の資格が、絞り込みの結果に依存している。

- 走査の範囲: 長さの要件が無いとき、AI facet の全体取得は `DetailsMetaScanOrder::CurrentGrid` で、既に絞り込まれた `current_grid_order()` だけを走査する (`src/app.rs:74931-74938`)。
- 完了の資格: `ai_model_facet_ready()` (`src/app.rs:74631`) は、次のときに真。
  - context で 1 つの `details_image_dims_state` が `Ready` か `Reconciling`
  - または `VisibleStage` の取得中

  どの行の集合について取得したかは持たない。
- 判定: ready のとき、AI の値が無い画像は「該当なし」で除外する (`src/app.rs:76713-76736`)。

その結果、候補の集合が増えると、増えた画像は未取得のまま消える。

今は、facet の変更ごとの全体の読み直しが、要件と状態を立て直している (`src/app.rs:46064-46072`)。そのため、facet の変更では表に出ていない。

- 例 1 (r2): サイズで画像 B を除外 → AI メニューで A を取得 → AI 条件を選択 → サイズを解除。key から facet を外すと、B が消える。
- 例 2 (r1): 親で AI 条件 → 子フォルダへ移動して退避 → 子で寸法の取得が完了 → 退避バッジで復元 (`restore_facet_filter_suppression`)。未取得の行が消える。
  - 復元は描画中に `rebuild_visible_indices()` まで進む。そのため、poll で要求を立て直しても、既に絞られた集合を走査し直すだけになる (r2 P2-1)。
- 例 3 (既存の不具合、コードからの推論・実機未確認): 評価フィルタは元から key に無い。そのため v5.5.0 でも、「評価で B を除外 → AI 条件 → 評価を解除」で B が消えるはず。
- `ai_model_facet_requested` が真でも、今の候補の集合について取得済みであることは示さない (r2 P2-2)。

#### 4.2.2 直し方: 長さ (§1.762) と同じ形にする

長さは、すでに「一覧全体の走査 + 取得の証明」を持つ (`docs/media-duration-sort-filter-plan.md` §5)。

- 対象: 表示集合・絞り込み・可視範囲に依らず、`items` 全体 (`DetailsMetaScanOrder::WholeListing`、`src/app.rs:74899-74905`)。
- 完了: `DurationScanProof { generation, items_len }` (`src/app/duration_metadata.rs:6`) が context ごとに立つ。
- 判定: 証明が今の一覧と一致するまでは、長さ条件を適用しない (分からない行は表示したまま)。

AI facet も、この形に乗せる。

- 持ち主は、今と同じ details meta (`poll_details_meta_load` / `start_details_meta_load_for_scope` / `finish_details_lazy_session`) のまま。
- 新しい worker や owner は作らない。

手順:

1. **証明を、一覧全体の要件の証明に広げる。**
   - `DurationScanProof` を、何の要件について全体を取得し終えたかを持つ証明に置き換える。名前は実装で決める。例: `ListingMetaScanProof { generation, items_len, duration: bool, ai: bool }`。
   - 走査の開始時に、要件の snapshot を取る (`duration = duration_lengths_wanted()`、`ai = ai_model_facet_should_load()`)。
   - 完了時に証明を保存する。同じ `generation` / `items_len` の証明が既にあれば、要件は和をとる (取得済みの要件を、後の走査で失わない)。
   - 一覧の採用で `None` に戻す規則 (`src/app.rs:46068`) は、今のまま。
2. **判定を証明から導く。**
   - `duration_scan_ready()`: 証明が一致し、`duration` が真。
   - `ai_model_facet_ready()`: 証明が一致し、`ai` が真。
   - `details_image_dims_state` の `Ready` / `Reconciling` / `VisibleStage` から AI の完了を推すのをやめる。
   - 呼び出し側 (判定 `:76713`、AI メニュー `src/ui_main.rs:14715`・`:14764`) は、この判定を使うだけで変えない。
3. **走査の範囲を、要件から決める。**
   - `AllRequirements` で `whole_listing_meta_scan_wanted()` (AI facet か長さ) が真なら、`WholeListing` で走査する。今は長さのときだけ。
   - AI facet の `CurrentGrid` の分岐は無くなる。
4. **画面外の行で取るものを、要件に揃える。**
   - 今の `details_meta_target_for_idx_with_scope` の `duration_only` 引数は、「表示集合の外の行は長さだけ取る」という意味。
   - これを「表示集合の外の行は、一覧全体の要件のフィールドだけ取る」に一般化する。
     - 長さが要件なら、動画・音声の長さ
     - AI が要件なら、AI メタデータ (`lazy_load_ai_metadata_for_idx`)
   - 作成日時・寸法・ページ数は、今と同じく表示集合の中だけで取る。
   - 「表示集合の外」は `visible_indices` の外のこと。スクロールの画面外ではない。表示集合の中の既存 helper による要求と、選択情報の対象の例外 (`src/app.rs:75099`) は保つ。
   - AI だけが要件のとき、全体要件だけの行で動画の probe を要求しない。
5. **立て直しを、要件の持ち主 1 か所で行う。**
   - 以下、「未充足の要件」= 今の一覧全体の要件から、今の一覧と一致する有効な証明が満たす要件を除いたもの。
   - poll の長さの立て直し (`src/app.rs:74086-74092`) を、「未充足の要件がある」に一般化する。条件は、取得中でないこと、`Cancelled` でないこと。
   - 取得中の走査は、次のどちらかで取消して始め直す。
     - 走査の証明の snapshot が、今の一覧と食い違う (`:74076-74080`)。
     - 未充足の要件を、取得中の走査の snapshot が含まない (取得中に AI facet が有効になった)。この条件は今回足す (r3 P2-2)。
       - VisibleStage の取得は全体の snapshot を持たない。有効な証明がすべての全体要件を満たしていれば、未充足の要件は空なので取消さない。
       - 今の要件の集合そのものと比べると、VisibleStage を取消し続ける。
   - `whole_listing_meta_scan_wanted()` / `duration_lengths_wanted()` は継続条件のまま。「未取得がある間だけ」に変えない。そうすると既存の継続条件と停止の保持を壊す (`media-duration-sort-filter-plan.md` §5.1)。
   - facet を変える入口は個別に直さない (メニュー・退避からの復元・お気に入り・スマートフォルダ・設定の取り込み・評価フィルタ)。
6. **完了時の作り直しを揃える。**
   - `finish_details_lazy_session` は今、AI facet が有効なら常に表示集合を作り直す (`:73972-73979`)。
   - これを、証明で AI か長さの判定が新たに使えるようになったときに作り直す形に揃える。今の長さの `duration_completed` と同じ形。
7. **段階取得を許すのは、全体の要件がすべて証明されてから (r3 P2-1)。**
   - `details_lazy_uses_visible_stages()` (`src/app.rs:73926`) は今、長さの証明だけを見る (`!duration_lengths_wanted() || duration_scan_ready()`)。これを「未充足の要件が無い」に一般化する。
   - これで、全体走査の `Finished` (`src/app.rs:74313-74336`) は、未充足の要件がある間は `Reconciling` に入らず、全体走査の snapshot を必ず `finish_details_lazy_session` へ渡して証明を立てる。
   - 今のままだと、AI だけを要求していてサムネイルの長さ表示か詳細のページ数列が有効な場合、`Reconciling` に入って snapshot を捨てる。post-grid の完了 (`:74014`) も snapshot なしなので、AI の証明が立たず、再走査を繰り返す。
   - 証明が立った後は、既存の VisibleStage と post-grid の完了をそのまま許す。
8. **空の一覧**: 取得の受付 `details_lazy_columns_visible()` (`src/app.rs:74397-74399`) は、空の一覧では長さの要件だけを例外にしている。これを一覧全体の要件に揃え、空の一覧でも AI の証明が即時に立つようにする。全件 cache の当たりのときの即時完了 (`launch_details_meta_worker` の `plan.total == 0`) も、証明を立てる。
9. **AI メニューの要求は、証明があれば印だけ立てる (r4 P2)。**
   - `request_ai_model_facet_load()` (`src/app.rs:74606`) は今、`requested` が偽なら、証明を確かめずに `invalidate_details_meta_requirements()` を呼ぶ。これは `Cancelled` も `NotRequested` に変える (`:74379`)。
   - AI のメニュー (`src/ui_main.rs:14713`・`:14762`) は表示のたびにこれを呼ぶ。そのため、次の順で、停止した長さの走査が、利用者の再開なしに動き出す。
     1. 退避から AI 条件を復元する。
     2. メニューを開かずに、AI の全体取得が終わる。`requested` は偽のまま、AI の証明は立つ。
     3. 長さの取得を始めて、停止する。
     4. AI のメニューを初めて開く。
   - 長さの `request_duration_facet_load` (`src/app/duration_metadata.rs:140`) と同じく、有効な AI の証明があれば `requested` だけを立て、取得の状態を無効にしない。

#### 4.2.3 これで閉じる経路

- 例 1・3: 走査は items 全体なので、ほかの絞り込みや評価フィルタで外れていた画像も取得済みになる。解除しても消えない。
- 例 2: 復元の時点で、証明に `ai` が無い。
  - そのため AI 条件は適用されず、行は消えない。
  - poll が全体の走査を始め、完了で作り直す。
- `requested` は「メニューを開いた」の印のまま。完了の資格には使わない。

#### 4.2.4 変わる挙動と費用

- AI facet を使う間は、ほかの絞り込みで外れている画像の AI メタデータも読む。
  - 読むのは、一覧の世代・件数・要件が変わったときの、未取得の分 (cache の当たりは読まない)。
  - 「世代ごとに 1 回」は厳密な上限ではない。件数が変わるか要件が足されると、全体を走査し直す。
  - 今の「facet を変えるたびにフォルダ全体を読み直す」が無くなるので、合計では軽くなるはず (未計測)。
- AI facet の読み込み中の表示は、表示集合ではなく、一覧全体の進み具合になる。
- 取得の停止 (`Cancelled`) は、長さと AI で同じ走査を止める (r3 P2-3 で訂正)。
  - 止めている間も、有効な証明と、その条件の適用は保つ。
  - 適用しないのは、未証明の要件の条件だけ (分からない行は表示したまま)。例: 長さを取得済みでページ数の VisibleStage を止めても、長さ条件は効いたまま。
  - 長さの既存の停止の契約 (`src/app.rs:74363`、`src/app/tests/duration_762/visible_stages.rs:129`) と同じ。
- 利用者の判断: 案 A として、上の費用を含めて選ばれた (統括経由、2026-11-21)。

#### 4.2.5 ほかの facet の要件 (r1 の照合のまま)

いずれも、読み直しに頼っていない。

- 長さ: 上の証明。
- ブックマーク: `rebuild_visible_indices` → `ensure_bookmark_presence_loaded` (`src/app.rs:75866` → `:54202`)。
- 編集の roll-up: 変換書庫の分類の入口 (`src/app.rs:58446`・`:58691`)。

### 4.3 detached / 複数 context

- `viewer_context_registry.rs:5159` の fork は、`folder_projection_key(&self.settings, …)` を呼ぶだけ。コードは変えない。
- facet は App 共有の `settings` にあり、context ごとの値ではない。外しても、context 間で key が facet によって食い違うことが無くなるだけ。
- 一覧全体の要件の証明 (§4.2) は、今の `duration_scan_proof` と同じ、viewer context ごとの状態として持つ。
  - `viewer_context_registry.rs` では、既存の登録 (構造体・初期化・swap・fork) の型を置き換えるだけで、所有の分類は変えない。
  - fork 先は空、元 context は証明を保持する (§1.762 と同じ)。
- detached の述語・viewport の選択・配送・HWND・placement・bundle の所有と交換は触らない。

## 5. 採らなかった案

- **範囲指定の DragValue を「確定時だけ値を反映」(`update_while_editing(false)` など) にする案。**
  - メニューは閉じなくなるが、facet を変えるたびに全体を読み直すこと自体は残る。大きいフォルダでは、毎回の読み直しの費用も残る。
  - 原因の key を直さずに、入口の 1 つだけを避ける症状対処。
- **facet の変更時だけ、監視の作り直しを抑える分岐を足す案。** key の意味 (行の記録の条件) を直さずに、例外を増やすだけ。
- **AI facet が有効なときだけ、key に facet を残す案。**
  - AI の取得不足を、読み直しで隠し続けるだけ。
  - AI 条件と範囲指定を一緒に使うと、範囲指定の件も残る。
- **AI の取得を「どの候補の集合について取得したか」で記録し、集合が増えたら追加で取る案。**
  - 集合の記録と比較という状態が増える。
  - 長さと同じく全体を取るなら、集合を記録しなくてよい。
- **poll で `request_ai_model_facet_load()` を呼び直すだけの案 (r2)。** 既に絞られた集合を走査し直すだけで、取得の対象が絞り込みに依存したまま残る (r2 P2-1・P2-2)。

## 6. 状態の組み合わせを減らす検討・まれな失敗

CLAUDE.md「設計の簡素化」に沿って、組み合わせそのものを減らせるかを先に検討した。

- **採った: 1 つの所有者・1 つの証明にまとめる。**
  - 今の AI の完了は、次の組み合わせで決まっている。
    - `details_image_dims_state` の 3 つの状態 (`Ready`・`Reconciling`・`VisibleStage` の取得中)
    - 走査範囲 (`CurrentGrid`) と表示集合
  - r3 では、長さと AI が同じ証明・同じ全体走査を使う。AI の完了は「証明が一致し、`ai` を含む」の 1 つになる。
  - 表示集合・可視範囲の stage・評価フィルタとの組み合わせを考えなくてよくなる。
- **採った: 確定の時点を 1 つにする。** facet を変える入口ごとに要求を立てず、details meta の poll 1 か所で、要件と証明を照らし合わせる。
- **採った: 既存機能と同じ経路に揃える。**
  - 長さ (§1.762) の全体走査・証明・停止と再開・「分からない間は適用しない」に、AI をそのまま乗せる。
  - 新しい規則は作らない。
- **採らなかった: 取得中はモーダルにする。**
  - 一覧全体の AI メタデータの取得は、数千件だと長くかかる。その間の閲覧を止めるのは、通常の使い方の退行になる。
  - 長さの取得も非モーダル。
- **採らなかった: facet の変更で一覧を閉じて作り直す。** それが今の全体の読み直しで、今回の不具合そのもの。
- 状態の数:
  - key の材料は 1 つ減る。
  - 証明は既存の型を置き換えるだけで、App や context の状態は増えない。
- まれな失敗:
  - 既定の割り切り方針による (ログ + 通知)。新しい失敗経路は無い。
  - 取得の失敗・取消は、既存の長さの扱い (`RetryLater`・`Cancelled`) と同じ。

## 7. テスト (実装の前に赤で足す)

本物のフレーム (`HeadlessEpiIntegration::update`) で確かめる。

- `src/app/tests/typing/` の道具 (§1.832) は、`probe::fields()` (`ime_focus` 経由の TextEdit) を前提にしている。DragValue の中の TextEdit は、`ime_focus` を通らない (`vendor/egui/src/widgets/drag_value.rs:560`)。
- そのため、道具の共通部分 (フレームの回し方、完了の保持と解放) が使えるなら使い、欄の特定は実ウィジェットの response から行う。
- 道具に乗せられない部分は、理由をテストのコメントに書く。

コミットは次の順に分ける。設計担当が各コミットでテストを流し、赤・緑を記録する。

1. テストだけ
2. key から facet を外す
3. AI の証明 (§4.2)

| # | テスト | v5.5.0 (テストだけ) | key だけ | r3 全部 |
| --- | --- | --- | --- | --- |
| T1 | 長さの範囲指定に 1 文字ずつ打つ | 赤の見込み | 緑 | 緑 |
| T2 | サイズの範囲指定に 1 文字ずつ打つ | 赤の見込み | 緑 | 緑 |
| T3 | 4 区切り・範囲指定の選択・チェック・拡張子で読み直さない | 赤の見込み | 緑 | 緑 |
| T4 | AI 条件のまま、ほかの条件 (サイズ) を緩める | 結果は緑・読み直し無しの検査は赤の見込み (読み直しで取得する) | 赤の見込み | 緑 |
| T5 | AI 条件のまま、評価フィルタを解除する | 赤の見込み (既存の不具合) | 赤の見込み | 緑 |
| T6 | 退避からの復元で AI 条件が戻る (r1 の例) | 実測で記録 | 赤の見込み | 緑 |
| T7 | 長さ preset の復元、メニューを経ない適用 | 実測で記録 | 実測で記録 | 緑 |
| T8 | 自動更新が今までどおり働く | 緑 | 緑 | 緑 |

各テストの中身:

- **T1** (長さの範囲指定):
  - 監視・初回 hydration・長さの取得を、先に終わらせる。
  - メニューを開き、「範囲指定」を選び、下限にチェックを入れる。
  - 実際にクリックして DragValue の編集モードへ入り、`Event::Text("2")`・`Event::Text("5")` を別フレームで送る。
  - 各段階 (「範囲指定」の選択、チェック、各文字) の後で、裏の処理が落ち着くまでフレームを回し、次を確かめる。
    - メニューが開いたまま
    - 欄にフォーカスがあり、文字が欄に入っている (最後に 25)
    - `items_generation` が進まない
    - 再走査が「不一致 → 読み直し」に進まない
    - sidecar restore のモーダル扱いに入らない
    - キーが一覧へ届かない (選択が動かない、何も開かない)
  - 修正前は、「範囲指定」の選択やチェックで先に赤になることがある。
- **T2**: T1 と同じことを、サイズの範囲指定で行う。
- **T3**: 長さ・サイズの 4 区切りと拡張子を切り替える。`items_generation` が進まず、visible_indices が期待どおりであることを確かめる。
- **T4** (AI 条件 + ほかの条件の解除):
  - AI 条件とサイズ条件を付け、サイズの外に、AI 条件に合う画像 B を置く。
  - AI の取得を完了させてからサイズを解除し、B が表示されることを確かめる。
  - 取得が要るなら、読み直し (`items_generation` の前進) なしで取得されること。
  - 取得が終わる前も、B は表示されたままであること (分からない行を消さない)。各フレームで確かめ、一時的な消失も赤とする。
  - 記録は 2 つに分ける: 最終結果の正しさ (B が出る) と、読み直し無し (`items_generation` が進まない)。
- **T5**: T4 と同じことを、評価フィルタの解除で行う。
- **T6** (退避からの復元):
  - 親で AI 条件を付け、子へ移動して退避し、子で寸法など AI 以外の取得を完了させてから復元する。
  - 未取得の画像が消えないこと。
  - 全体の走査が始まり、完了後に AI 条件が効くこと。
- **T7**: 長さ preset の復元でも、全体の走査が始まり、完了後に効くこと。メニューを開かずに適用した場合も同じ。
- **T8** (自動更新):
  - facet を入れたまま、ファイルを足す・消すと、通知で読み直されて結果に反映されること。
  - facet の変更前に始まった再走査の結果 (Equal / Different) を、変更後に受理しても、visible_indices が今の facet と一致すること。完了の配送を保持し、facet を変えてから解放すると決定的に試せる。

追加の検査:

- 証明について:
  - 取得中に AI facet が有効になったら、走査を始め直す。
  - 長さだけの証明の後に AI が要件になったら、全体を取り直す (cache の当たりは読まない)。
  - 要件の和をとる。
  - 一覧の採用で消える。
- 画面外の行について:
  - 長さだけの要求で、AI・作成日時・寸法・ページ数を取らない (`media-duration-sort-filter-plan.md` §5.1 の既存の検査を弱めない)。
  - AI だけの要求で、画面外の行は AI メタデータだけを取る。
- context について: 証明の swap・fork・bundle の破棄。§1.762 の `viewer_context_registry.rs` の検査の型を置き換える。
- 段階取得との共存 (r3 P2-1・P2-2):
  - AI だけを要求し、サムネイルの長さ表示か詳細のページ数列を有効にして、AI の全体走査の実際の `Finished` から証明が立つこと。
  - 証明が立った後の VisibleStage が、取消されずに完了すること。
  - 片方の証明 (例: 長さ) を保ったまま、他方 (AI) の走査を停止・再開できること。停止中も、証明済みの条件が効いたままであること。
- 即時の完了: 空の一覧と、全件が cache の当たりの一覧で、AI の証明が立つこと。
- 停止の保持 (r4 P2): 「AI の証明あり・`requested` 偽・長さ未証明・`Cancelled`」の状態で、AI モデルと生成ツールのメニューを開いても、停止が保たれること。
- T7 は既存の長さの契約の回帰検査で、修正前に赤である必要はない。

既存のテストは弱めない。

- `section375_tests.rs`・`egui037_connection_tests.rs:850`・`tests/duration_762.rs:421` の key 一致の検査は、そのまま通るはず。
- AI facet の既存のテストで、`CurrentGrid` や `Ready` からの推論に依存するものは、指示書に列挙したうえで書き換える。弱めない。

## 8. 文書

- `docs/section375-operation-reconcile-plan.md` §3.3: 「sort／facet変更で旧要求を拒否する」を「sort 変更で」に直し、facet を外した理由 (§2・§3) を足す。
- `docs/media-duration-sort-filter-plan.md` §5.1・§5.2: AI facet は「表示集合全体」、証明は長さだけ、と書いてある。AI も同じ証明に乗ったことに揃える。
- `docs/detached-rework-plan.md` §11: 触れた範囲と判断理由、統括と独立レビューの合意を記録する。
- `docs/async-architecture.md` / `docs/architecture-overview.md`: key の材料や AI facet の取得を書いた所があれば揃える。
- マニュアル: AI の絞り込みの読み込み中の説明 (件数の母数) を確かめ、要れば直す。範囲指定の説明は変わらない。

## 9. 段階 3 で書くテスト (段階 1 の引き継ぎ)

段階 1 は現行 API で観測できる契約を `src/app/tests/facet_835.rs` と配下に追加する。
製品コード・既存テストの期待値は変更せず、cargo による実行もしない。以下は新しい証明型と
要件 snapshot の形が決まってから書く。`#[ignore]` や仮の型・製品 API は追加しない。

- **証明の要件の和 (§4.2.2-1)**: 同じ generation / items_len で、長さ→AI、AI→長さの
  各完了が先の要件を保持する。異なる generation / items_len の証明とは和をとらない。
  現行 `DurationScanProof` には AI 要件を表す欄が無い。
- **要件 snapshot と資格 (§4.2.2-2・5)**: 要件追加時の取消・再走査の挙動は段階 1 で
  検査する。段階 3 では、旧 snapshot の Finished が新しい AI 要件を証明しないこと、
  長さだけの証明 / AI だけの証明 / 両方の証明を区別した未充足要件の差集合も検査する。
- **一覧採用での破棄 (§4.2.2-1)**: 段階 1 は既存 ready API で採用後の無効化を検査する。
  段階 3 では統合証明自体が None になることと、旧一覧の遅着 Finished を検査する。
- **context 所有 (§4.3)**: `viewer_context_registry.rs` の §1.762 の型依存テストを統合証明へ
  置き換え、長さと AI の要件を含めた swap、fork 先は空・元は保持、bundle 破棄で兄弟を
  失効させないことを検査する。段階 1 では既存の context テストを変更しない。
- **段階取得との共存 (§4.2.2-7)**: AI だけ + サムネイルの長さ表示、および AI だけ + 詳細の
  ページ数列の 2 経路で、実 worker の Finished を保持・解放し、全体の snapshot が証明へ
  渡ったことを検査する。Ready / Reconciling だけを見て代用しない。証明成立後の本物の
  VisibleStage が取消されずに完了し、既存の証明を上書きしないことも検査する。
- **即時完了 (§4.2.2-8)**: 空一覧と全件 cache hit は段階 1 で既存の取得・ready API から
  検査する。段階 3 では即時完了の統合証明に AI 要件が入ることを直接検査する。
- **停止と資格 (§4.2.4・§4.2.2-9)**: 長さの証明を保った AI の停止・再開、および
  requested 偽の実 AI 取得→長さの停止→両 AI メニューの初回 open は段階 1 で検査する。
  段階 3 では両方の順序で部分証明そのものの保持と、停止中の未証明要件だけが不適用に
  なることを確認する (まれな I/O 失敗ごとの回復テストは追加しない)。

段階 1 の source inspection で分かった設計表との相違:

- T8 の通知による追加・削除は現行でも緑の見込み。一方、facet 変更をまたぐ旧 Equal /
  Different の**受理**は、現行 `proof_valid` が facet を含む key を照合して拒否するため、
  テストだけの段階では赤の見込み。§7 の T8 一行全体を「緑」とする予測とは異なる。
- `restore_facet_filter_suppression()` 自体は settings と名前欄の runtime を戻すだけ。
  復元バッジの呼び出し側が直後に `rebuild_visible_indices()` を呼ぶ。T6/T7 はこの両方を
  実行し、poll より前の投影も検査する。
- 既存の app AI facet テストは主に `DetailsLazyMeta` / Ready を直接設定している。
  T4〜T6 はそれを使わず、既存 `agent_ipc/item_info_tests.rs` と同じ parameters text chunk の
  PNG を作り、本物の details meta worker の取得結果から取得済み / 未取得を区別する。
- T1/T2 の選択・チェック時点は編集モードへ入る前なので、欄の表示・有効状態・値を検査する。
  実クリックによる編集開始後は focus・TextEdit cursor・描画された draft も毎フレーム検査する。
  `DragValue` の numeric accessibility 値は入力処理前の値を報告するため、当該フレームの
  文字列は内部 TextEdit が出す value で確かめる。

## 10. 実行結果の記録

テストは設計担当 ClaudeCode が共有の実行口 (run-heavy) で流した。実機では確かめていない。

### 10.1 段階 1: テストだけ (製品コードは v5.5.0 相当、fc14804bf、2026-11-21)

`cargo nextest run -p mimageviewer --lib -E 'test(/facet_835_/)'`: 21 本中、緑 5・赤 16。

| テスト (`facet_835_` を省略) | 結果 | 失敗の内容 |
| --- | --- | --- |
| `t1_duration_range_types_each_character_in_real_frames` | 赤 | 「範囲指定」を選んだ段階でメニューが閉じた (`1 menu closed`、generation=2) |
| `t2_size_range_types_each_character_in_real_frames` | 赤 | 同上 (サイズ) |
| `t3_duration_presets_only_project_visible_rows` | 赤 | 区切りの選択で一覧の世代が 1 → 2 |
| `t3_size_presets_only_project_visible_rows` | 赤 | 同上 |
| `t3_extension_changes_only_project_visible_rows` | 赤 | 拡張子の選択で一覧の世代が 1 → 2 |
| `t4_size_relaxation_final_result_and_no_transient_omission` | 赤 | サイズの解除後、取得中のフレームで B が消えた |
| `t4_size_relaxation_does_not_reload_listing` | 赤 | 世代が 3 → 4 (読み直しで取得している) |
| `t5_rating_relaxation_keeps_matching_image_without_reload` | 赤 | 評価フィルタの解除後、条件に合う B が出ない (既存の不具合をテストで再現) |
| `t6_restore_ai_after_child_dimensions_does_not_hide_unread_images` | 赤 | 復元の直後、未取得なのに AI 条件が適用されて 3 件とも消えた |
| `t7_restored_duration_preset_acquires_without_menu` | 緑 | |
| `t7_direct_duration_preset_acquires_without_menu` | 緑 | |
| `t8_notifications_add_and_delete_with_active_facet` | 緑 | |
| `t8_equal_started_before_facet_change_is_accepted` | 赤 | facet の変更をまたいだ Equal が拒否された |
| `t8_different_started_before_facet_change_uses_current_facet` | 赤 | 同じく Different が拒否・先送りされた |
| `duration_completion_cannot_stand_in_for_ai_completion` | 赤 | 長さだけの完了で AI 条件が適用された |
| `ai_becoming_required_restarts_inflight_duration_scan` | 赤 | 取得中に AI が要件になっても、走査を始め直さない |
| `ai_only_plan_reads_hidden_images_without_unrelated_fields` | 赤 | AI の走査が `current_grid_order` の外を読まない |
| `ai_cached_completion_and_empty_listing_are_ready` | 緑 | |
| `ai_menus_preserve_stopped_unproven_duration_after_direct_ai_acquisition` | 赤 | AI メニューを初めて開くと、停止した走査が動き出した (`Cancelled` → `Loading`) |
| `duration_only_hidden_targets_do_not_acquire_ai_or_other_columns` | 緑 | |
| `stopping_and_resuming_ai_keeps_proven_duration_filter` | 赤 | 長さ条件で隠れた画像の AI を取得しない |

- T1・T2 は、サブ PC 担当が観測した「メニューが閉じる」をテストで再現している。1 文字目より前の「範囲指定」の選択で閉じる。
- T5 は、設計 §4.2.1 の例 3 (評価フィルタ、v5.5.0 にもある不具合) をテストで再現している。
- 緑の 5 本は、既存の長さの契約と通知の経路で、修正後も緑のままであること。
