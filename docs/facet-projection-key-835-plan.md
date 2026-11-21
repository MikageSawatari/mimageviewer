# §1.835 絞り込みの変更で一覧を読み直さない — 自動更新の判定 key から facet を外す (設計)

- 状態: 設計 r2 (2026-11-21)。r1 の独立レビュー (Sol xhigh) は REVISE・構造的修正には AGREE。r2 で AI の要件 (§4.2)・参照の誤り・テストの赤の位置を直した。v5.5.1 で直す (利用者決定)。
- 書いた人: 設計担当 ClaudeCode。根拠はコードと、サブ PC 担当の観測ログ (`C:\home\mimageviewer-subpc\duration-762\results\`、リポジトリ外)。行番号は master `ebfa00b91`。

## 1. 観測と原因

- 観測 (サブ PC 担当、2026-11-21): 長さの範囲指定で数字を 1 文字打つと、その値で絞り込まれてメニューが閉じた。2 文字目以降は一覧に届き、Enter で選択中の動画が開いた。絞り込みや並べ替えを変えるたびに、ログに `auto-refresh: folder content changed (…), reloading` が出た (17 回)。ファイルは変わっていない (SHA-256 で照合)。
- 経路 (コード):
  1. 絞り込みを変えると `settings.facet_filter` が変わる。
  2. 監視の点検 (`src/app.rs:27774-27788`) は、`current_folder_projection_key` が監視作成時の key と違うと、監視を作り直して `start_external_rescan` を呼ぶ。
  3. key を作る `folder_projection_key_with_presentation` (`src/app.rs:5076`) の材料に `"facet": settings.facet_filter` がある。
  4. `start_external_rescan` は baseline を同じ key の観測に限る (`src/app.rs:27985-27993`)。baseline が無いので結果は「一致」にならず、`auto-refresh … reloading` から全体を読み直す。
  5. 読み直しの sidecar restore 中はモーダル扱い (`modal_dialog_block_reason_filtered` の `"sidecar_restore"`) になる。`note_main_chrome_pointer_block_transition` が `egui::Popup::close_all` を呼んでメニューを閉じ、キーを保留する。
- 長さとサイズの範囲指定 (チェック + DragValue。DragValue は編集中も 1 文字ごとに値を更新する) では、1 文字目でメニューが閉じる。ほかの絞り込みも変えるたびにフォルダ全体を読み直しているが、1 回選んで閉じる項目なので目立たない。
- 所有: key の行は `a73ea3750` (2026-10-14、§1.375 R10) から。サイズの範囲指定は v5.4.0 でも同じ経路を通る (コードからの推論、実機未確認)。

## 2. 守るべき不変条件

自動更新の観測 (`FolderObservation`) は、走査・materialize が作った物理行の記録 (`FolderRowRecord`) と、その行を作った投影条件を対で持つ。key は「この行の記録を作った条件」を識別するためのもので、条件が変われば旧記録を比較に使えない。

- §1.375 の記録の契約 (`docs/section375-operation-reconcile-plan.md` §3.1・§8.1) で、行の記録の境界は「種類採否・同名 winner・補助画像・既存 rating sort と表示 block 順まで」。facet と details は「items 配列ではなく既存 visible_indices / details_order の投影を保持する」(同 §8.1)。
- つまり facet は行の記録の外にあり、行を作る条件ではない。

## 3. 違反を作っている箇所

key に facet が入っているので、行を作る条件が同じでも key が変わる。そのため、facet の変更が「行を作る条件の変更」として扱われ、ファイルの変更が無いのに全体の読み直しが走る。

- 行を作る側は facet を読まない (`src/app/book_materialization.rs`・`src/app/folder_scan.rs`・`src/grid_item.rs`・`src/rating_sort.rs` に `facet` の参照 0 件)。facet は `rebuild_visible_indices` → `passes_facet_filter` (`src/app.rs:75759`・`:76668`) だけで効く。
- 同じく表示だけの絞り込みである評価フィルタ (`settings.rating_filter`) は、元から key に無い。facet の中でも、`name_query` と `place_keys` は `#[serde(skip)]` のため、元から key に入っていない。(どちらも key に無いという事実だけで、それが正しく動くことの実証ではない。)
- §1.375 計画 §3.3 は「sort／facet変更で旧要求を拒否する」と書いている。facet を入れた理由は書かれていない。行の記録の契約 (§2) とは食い違っている。

## 4. 修正

- `folder_projection_key_with_presentation` の材料から `"facet"` を外す。外した理由のコメントも書く。
- §4.2 の AI の要件を、読み直しに頼らず要件の持ち主で立てる (r2 で追加)。
- 外す理由のコメント: facet は行の記録の後の表示投影 (visible_indices) で、`rebuild_visible_indices` が今の設定で作り直す。行の記録の key に入れると、facet の変更でファイルが変わっていないのに全体を読み直す。
- 残すもの: `sort`・`order` (grid_display_order = 表示 block 順)・`visibility` (種類採否)・同名本・skip 系・`autofs`・`books`。いずれも行の記録の境界の内側。stack 関係は生の `FolderRowRecord` の条件ではないが、記録から決定的に作る group 表示の条件で、保持される group 表示に要る (`src/app/filename_stack_ui.rs:187`・`:1319`)。key は「行の記録と、そこから決定的に作る group の条件」を識別する。
- key を使う箇所は全部同じ関数を通る。関数を変えれば、全箇所で一律に意味が変わる。
  - 監視の作り直し: `src/app.rs:27774`
  - 再走査の baseline: `src/app.rs:27973`
  - worker の一致判定: `src/app.rs:28101`
  - 結果を捨てるかの判定 `proof_valid`: `src/app.rs:28219`
  - 一致の採用: `src/app.rs:28342`
  - 観測の作成: `src/app.rs:28349`・`:32560` ほか
  - テスト用の補助 (`#[cfg(test)]`、製品経路ではない): `src/app.rs:28408`・`:28440`
  - detached の fork: `src/app/viewer_context_registry.rs:5159`
- 呼び出し側は変えない。

### 4.1 facet を変えた後に受理される旧要求

外すと、facet の変更前に始まった再走査の結果を、変更後に受理することになる (`proof_valid`)。

- 一致 (Equal) の場合: items をそのまま保つ。visible_indices は facet の変更時に今の設定で作り直してある。
- 不一致 (Different) の場合: 通常の load で install し、visible_indices は今の facet で作り直す。

実装時に、Different の load が要求時の facet を持ち込まないことをコードで確かめる。

### 4.2 読み直しのついでに立っていた取得の要件

これまでの読み直しは、一覧の採用時 (`src/app.rs:46066-46067`) に、facet に要るデータの取得の要件を今の facet から立て直していた。

```
ai_model_facet_requested = facet_ai_filter_active()
duration_facet_requested = duration_preset.is_some()
```

読み直しが無くなると、メニューを経ない facet の適用では、この立て直しが無くなる。facet ごとの状況 (r1 レビューの照合と ClaudeCode の確認):

- 長さ: 読み直しに頼らない。details meta の poll (`src/app.rs:74086-74092`) が、`duration_lengths_wanted()` (preset を含む) かつ `duration_scan_ready()` でないとき状態を `NotRequested` に戻し、全体の取得を立て直す。
- ブックマーク: 頼らない。`rebuild_visible_indices` から `ensure_bookmark_presence_loaded` (`src/app.rs:75866` → `:54202`)。
- 編集の roll-up: 頼らない。変換書庫の分類の入口 (`src/app.rs:58446`・`:58691`)。
- **AI モデル: 頼っている (r1 P2)**。
  - `ai_model_facet_should_load()` は要否を返すだけで、`request_ai_model_facet_load` (`src/app.rs:74606`) を通らないと、取得の要件が無効にならない (`invalidate_details_meta_requirements`)。
  - details の状態が既に `Ready` (表示範囲の段階取得の完了など) なら、全件の取得は始まらない (`src/app.rs:76039`・`:74127`)。
  - 到達例: 親で AI facet を設定 → 子フォルダへ移動して退避 → 子で寸法の取得が完了 → 退避バッジで復元 (`restore_facet_filter_suppression`、`src/app.rs:79264`)。
  - その結果、未取得の画像が非該当として消える。今は読み直しが要件を立て直しているので出ない。

修正:
- AI にも長さと同じ形を与える。details meta の要件の持ち主 (長さの立て直しと同じ poll) で、`facet_ai_filter_active()` で要件が有効なのに `ai_model_facet_requested` が立っていなければ、既存の `request_ai_model_facet_load()` を呼ぶ。
- この形は、一覧の採用時の規則 (`requested = facet_ai_filter_active()`) を facet の変更の時点にも及ぼすだけで、新しい状態は足さない。
- facet を変える入口 (メニュー・復元・お気に入り・スマートフォルダ・設定の取り込み) を個別に直さず、要件の持ち主 1 か所で受ける。
- 実装時に、長さの poll と同じ境界で順序が成り立つかを確かめる。要件を立てた同じ poll で取得を始めるか、次の poll か。
- 同じ確認を、facet を変えるほかの経路でも行う。facet を変える全入口を列挙し、AI・長さ・ブックマーク・roll-up のそれぞれで、読み直しなしで取得が始まることをテストで押さえる (§7)。

### 4.3 detached / 複数 context

- `viewer_context_registry.rs:5159` の fork は `folder_projection_key(&self.settings, …)` を呼ぶだけで、コードは変えない。
- facet は App 共有の `settings` にあり、context ごとの値ではない。外すと、context 間で key が facet によって食い違うことが無くなるだけ。
- detached の述語・viewport の選択・配送・HWND・placement・bundle の所有と交換は触らない。

## 5. 採らなかった案

- 範囲指定の DragValue を「確定時だけ値を反映」(`update_while_editing(false)` など) にする案。メニューは閉じなくなるが、facet を変えるたびに全体を読み直すこと自体は残り、大きいフォルダで毎回の読み直しの費用が残る。原因の key を直さずに入口の 1 つだけを避ける症状対処なので、採らない。
- facet の変更時だけ監視の作り直しを抑える分岐を足す案。key の意味 (行の記録の条件) を直さずに、例外を増やすだけなので採らない。

## 6. 状態の組み合わせ・まれな失敗

- 状態は増やさない (key の材料が 1 つ減るだけ)。
- まれな失敗は既定の割り切り方針による。新しい失敗経路は無い。

## 7. テスト (実装の前に赤で足す)

本物のフレーム (`HeadlessEpiIntegration::update`) で確かめる。

- `src/app/tests/typing/` の道具 (§1.832) は `probe::fields()` (`ime_focus` 経由の TextEdit) を前提にしている。DragValue の中の TextEdit は `ime_focus` を通らない (`vendor/egui/src/widgets/drag_value.rs:560`)。
- そのため、道具の共通部分 (フレームの回し方・完了の保持と解放) が使えるなら使い、欄の特定は実ウィジェットの response から行う。
- 道具に乗せられない部分は、理由をテストのコメントに書く。

進め方:
- 監視・初回 hydration・長さの取得を先に終わらせる。
- 実際にクリックして編集モードへ入り、`Event::Text("2")`・`Event::Text("5")` を別フレームで送る。
- 各フレームで、モーダル・popup・focus・generation を観測する。
- 修正前は「範囲指定」の選択や下限のチェックでも key が変わる。そのため、最初の文字より前に赤になることがある。各段階ごとに検査する。

1. 長さの範囲指定: メニューを開き、「範囲指定」を選び、下限にチェックを入れて DragValue に 1 文字ずつ「2」「5」と打つ。各文字の後で次を確かめる:
   - 裏の処理が落ち着くまでフレームを回してから見る
   - メニューが開いたまま
   - 欄にフォーカスがあり、文字が欄に入っている (最後に 25)
   - `items_generation` が進まない
   - 再走査が「不一致 → 読み直し」に進まない
   - sidecar restore のモーダル扱いに入らない
   - キーが一覧へ届かない (選択が動かない・何も開かない)
2. サイズの範囲指定: 1 と同じ。
3. 4 区切りの絞り込み (長さ・サイズ) と拡張子の絞り込みを変えても、`items_generation` が進まず、結果 (visible_indices) が今までどおり。「範囲指定」の選択と下限・上限のチェックでも、メニューが閉じない。
3a. 取得の要件 (§4.2): メニューを経ない facet の適用 (退避からの復元を必ず含む) で、AI モデルの facet の全件取得が読み直しなしで始まり、未取得の画像が非該当として消えない。r1 の到達例を再現する。長さの preset の復元でも、長さの全件取得が始まる。
4. 自動更新が今までどおり働く:
   - facet を入れたまま、ファイルを足す・消すと、通知で読み直されて結果に反映される
   - facet の変更前に始まった再走査の結果 (Equal / Different) を変更後に受理しても、visible_indices が今の facet と一致する
   - 完了の配送を保持し、facet を変えてから解放すると決定的に試せる
5. 既存テストは弱めない。`section375_tests.rs`・`egui037_connection_tests.rs:850`・`tests/duration_762.rs:421` の key 一致の検査は、そのまま通るはず。

赤の確認: 1・2・3 は修正前に赤 (メニューが閉じる、または generation が進む)、修正後に緑であること。3a は、key だけを外した段階で赤になり (AI)、§4.2 の修正で緑になることを確かめる。

## 8. 文書

- `docs/section375-operation-reconcile-plan.md` §3.3 の「sort／facet変更で旧要求を拒否する」を「sort 変更で」に直し、facet を外した理由 (§2・§3) を足す。
- `docs/detached-rework-plan.md` §11 に、触れた範囲と判断理由を記録する。
- `docs/async-architecture.md` / `docs/architecture-overview.md` に key の材料を書いた所があれば揃える。
- マニュアル: 挙動の説明は変わらない (範囲指定は元から「数字を入れる」説明)。確かめて、要らなければ触らない。
