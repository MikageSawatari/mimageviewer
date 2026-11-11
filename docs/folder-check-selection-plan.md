# §1.732 どの一覧でもフォルダを複数選択・Ctrl+A の対象にする — 設計

- 状態: 実装済み (ブランチ feat-732-folder-select、実機確認待ち、2026-11-10)。設計の独立レビュー (GPT-6.1 Sol / xhigh、2026-11-10) は ACCEPT WITH CHANGES。指摘 P2×3・P3×2 を本書に反映済み (ブックマーク一覧の代理フォルダ行、Windows 標準の削除は対象外、外部ツールの件数への影響、判定式の純関数への集約、テストの追加)。
- 発端: メール報告 (報告者の観測)。レーティング一覧やコレクションで Ctrl/Shift+クリックでフォルダを複数選択できない (ファイルはできる)。どの一覧でも Ctrl+A でフォルダが選ばれない。
- 利用者決定 (2026-11-07): 仕様を変える。既存の動きが変わる点はリリースノート (必要なら「重要な変更点」) で知らせる。削除の確認・本へ追加・AI 説明文の扱いは下の §3 のとおり (削除の確認は利用者決定、本へ追加の ZIP と AI 説明文は本書で決めた)。

## 1. 現状 (コード、master c2abecc36)

### 1.1 選べるかどうかの判定

| 判定 | 場所 | フォルダ |
| --- | --- | --- |
| `GridItem::is_checkable()` | `src/grid_item.rs` | 含まない (画像・動画・音声・ZIP/PDF 本体・変換前書庫・ZIP 内画像・PDF ページ) |
| `App::grid_item_can_be_checked(idx)` | `src/app/subfolder_expansion.rs` | `subfolder_expansion_available()` が真のときだけ追加で認める |
| `grid_selection_item_is_checkable(items, allow_folders, idx)` | `src/ui_main.rs` | 呼び出し側 (`dispatch_grid_cell_input`) が `allow_folders = subfolder_expansion_available()` を渡す |

`subfolder_expansion_available()` は「実フォルダをそのまま表示している通常一覧 (本棚を含む)」だけで真。レーティング一覧・タグ一覧・閲覧履歴・アイテム検索・名前検索・お気に入り検索・スマートフォルダ・ZIP 内・スナップショット・検索バー表示中・ドライブ一覧では偽。この条件はサブフォルダ展開の起点選択のために f7d6a914c で入れたもので、選択一般の条件としてはそのまま借りているだけ。

### 1.2 選択の入口

| 入口 | 場所 | フォルダ |
| --- | --- | --- |
| クリック / Ctrl+クリック / Shift+クリック (サムネイル・詳細共通) | `ui_main.rs` `dispatch_grid_cell_input` → `select_grid_cell` → `apply_grid_click_selection` / `add_grid_selection_range` | `allow_folders` (= 展開可能な一覧だけ) |
| エクスプローラー方式の Ctrl+クリック | `apply_grid_click_selection` | 対象外の項目を Ctrl+クリックすると**チェック全体を消してその 1 件の単独選択**になる (報告の症状) |
| Shift+矢印・PageUp/Down・Home/End の範囲 | `app.rs` (キー処理) | `grid_item_can_be_checked` |
| Space | `app.rs` | `grid_item_can_be_checked` |
| Ctrl+A (`GridSelectAll`) | `app/update_phases.rs` | `is_checkable()` を直接見るので**どの一覧でもフォルダが入らない** |
| ゲームパッドの全チェック | `app/gamepad_input.rs` | `is_checkable()` を直接見る |
| ゲームパッドのチェック切替 | `app/gamepad_input.rs` `toggle_selected_grid_check` | `grid_item_can_be_checked` |
| 貼り付け・新しいフォルダ後の自動チェック | `app.rs` | `drag_source_path` から作るので種類を問わない (今もフォルダが入る) |
| 全画面の Space | `ui_fullscreen.rs` | 画像ページだけ (本件の対象外) |

ドラッグ範囲選択・選択の反転・メニューの「すべて選択」は無い。チェック印は種類を問わず同じ描き方 (`thumb_overlay_layout.rs` / `app/grid_paint.rs`) で、フォルダ用の描画追加は要らない。

### 1.3 選んだ項目を使う側 (フォルダがチェックに入ったとき、今の動き)

展開可能な通常一覧では今もフォルダをチェックできるので、下はすでに実際に通る経路。

| 操作 | 場所 | フォルダ |
| --- | --- | --- |
| 削除 (Delete / 右クリック) | `ui_dialogs/context_menu.rs` | 実フォルダとして中身ごとごみ箱へ。確認文に「フォルダの中には一覧に表示していないファイルも含まれます」。**「ごみ箱へ移すときは確認しない」ON だと確認も警告も出ない** (`should_skip_delete_confirmation`) |
| コレクション root の Delete / 「コレクションから外す」 | `app/collection_grid.rs` | 登録解除 (ファイルは消さない)。種類を問わない |
| ブックマーク一覧の Delete | `app.rs` | ブックマーク行の削除。ページが消えた・相対パスが不正なブックマークは、元の本 (製本・画像フォルダ) の `Folder` 行として並ぶ (`bookmark_browser.rs` の代理行)。Delete はこの行もブックマークだけを消す (`context_menu.rs` の分岐) |
| コピー / 切り取り / パスのコピー / D&D | `app.rs` / `context_menu.rs` / `ui_main.rs` | 含む (エクスプローラーと同じく中身ごと) |
| 名前の変更 | `app/rename_item.rs` | 2 件以上は断る。1 件ならフォルダも可 |
| ★ (F1〜F5 等) | `app.rs` `ratable_targets` | 含む (コンテナ★) |
| タグ | `tag_ops.rs` | 含む (tags.db) |
| コレクションへ追加 | `ui_dialogs/collections.rs` | 含む |
| フォルダへ移動 / コピー (整理) | `file_organize.rs` | 含む |
| 外部ツール | `external_tool.rs` | 実パス (`LaunchTarget::RealFile`) として含む |
| 本へ追加 | `ui_fullscreen.rs` `add_grid_selection_to_named_book` | **1 件でも追加できない項目があると全体を中止**。フォルダ・ZIP・PDF・変換前書庫・音声・動画 (専用の案内) がこれに当たる |
| AI 説明文の作成 / 削除 | `ui_dialogs/caption_job.rs` / `caption_delete.rs` | 本 (`BookKind::Folder`) として表紙を対象にする |
| 一括書き出し (Ctrl+E) | `ui_dialogs/export_batch.rs` | 対象外として数え、ダイアログに件数を出す |
| 編集内容の貼り付け / リセット | `edit_bundle_bulk.rs` | 対象外として数え、ダイアログに「対象外 M 件」 |
| 補正スロット・パラメータ解除・マスク/隠しスロット | `app.rs` `ratable_page_targets` | 黙って飛ばす (残りが 0 件なら断る) |
| 回転 (チェック時の右クリック) | `context_menu.rs` | 黙って飛ばす |
| EPUB → PDF / 書庫 → ZIP | `epub_batch_convert.rs` / `batch_convert.rs` | 種類で絞るので黙って飛ばす |
| サブフォルダ展開 | `app/subfolder_expansion.rs` `selected_subfolder_expansion_roots` | チェックしたフォルダだけを起点にする (展開可能な一覧だけ) |
| 選択数「N 個選択」・チェック数オーバーレイ・メニューの [N件] | `ui_main.rs` / `context_menu.rs` | 数える |
| 選択の「合計サイズ」 | `ui_main.rs` `selection_info_content` | フォルダの `image_metas` は一覧の種類によって `None` (行が消える) か `Some((mtime, 0))` (0 として足す) |
| 開く・スライドショー・比較 | — | `checked` を見ない |
| MCP / mIV Remote | — | `checked` を見ない |

一覧の種類による違いは、コレクション root (登録解除)・ブックマーク一覧・展開の起点・スマートフォルダの条件絞り込み (`smart_folder_rule_qualifies_index`) だけ。その他の操作は一覧の種類を問わず同じ。

## 2. 方針

### 2.1 判定を 1 か所にまとめる

- 式は**純関数 1 つ**に置く (例: `grid_item.rs` の `grid_item_checkable(item: &GridItem, folders_allowed: bool) -> bool` = `item.is_checkable() || (folders_allowed && matches!(item, GridItem::Folder(_)))`)。今は `App::grid_item_can_be_checked` と `ui_main.rs` の `grid_selection_item_is_checkable` が同じ式を別々に持っているので、両方ともこの純関数を呼ぶ形にする (設計レビュー P3)。
- 許可条件も 1 つにする: `App::grid_folders_checkable()` (新設) = `!self.items_are_drive_list`。ドライブ一覧はチェック・★・D&D・右クリックをすべて無効にしている一覧 (`docs/spec.md` の表) なので、そこだけ除く。その他の一覧の条件は足さない。
- `App::grid_item_can_be_checked(idx)` = `grid_item_checkable(item, self.grid_folders_checkable())`。App 側の入口 (Space・Shift+矢印・Ctrl+A・ゲームパッド) はすべてこれを使う。
- クリック系 (`dispatch_grid_cell_input`) が渡す `allow_folders` を `self.grid_folders_checkable()` にする。`ui_main.rs` の純関数群は引数の bool のまま (テストしやすさのため)。
- `checked` と `items_are_drive_list` はどちらも viewer context の bundle で入れ替わる (`app/viewer_context_registry.rs`) ので、判定は常に今の context の一覧に対して働く。context ごとの追加の状態は持たない。
- Ctrl+A (`update_phases.rs`) とゲームパッドの全チェック (`gamepad_input.rs`) を `is_checkable()` から `grid_item_can_be_checked(idx)` に替える。
- `GridItem::is_checkable()` の意味は「項目単独で決まる、フォルダ以外のチェック対象」のまま残す (doc コメントを、フォルダは App 側の判定で加わる、に直す)。名前を変えるかは実装者の判断でよいが、直接呼ぶ箇所を選択の入口に残さないこと。直接呼んでいる入口が残っていないことをテスト (または grep で確かめた一覧) で示す。
- `ZipDir` / `Stack` / `SearchContainer` / `CollectionPlaceholder` は本件の対象外 (今までどおり選べない)。

簡素化の検討 (CLAUDE.md): 一覧の種類ごとに許可を分ける案は採らない。使う側の操作は一覧の種類を問わず同じなので、許可を分けても状態の組み合わせが増えるだけになる。除くのは操作自体が無効なドライブ一覧だけにする。

### 2.2 サブフォルダ展開とのかかわり

- `selected_subfolder_expansion_roots()` は変えない (展開可能な一覧だけで働き、チェックしたフォルダを起点にする)。
- 結果として、通常フォルダで Ctrl+A の後にサブフォルダ展開を押すと「チェックしたフォルダだけ展開」になり、今のフォルダ直下のファイルが外れる。利用者決定済みの仕様変更として受け入れ、リリースノートに書く。

## 3. 使う側の操作ごとの挙動

変える操作は 3 つ (削除の確認・本へ追加・合計サイズ)。ほかは今のまま (§1.3 のとおり、展開可能な一覧で今すでに通る動きがどの一覧でも同じになるだけ)。

### 3.1 削除の確認 (利用者決定 2026-11-07)

- フォルダを 1 つでも含む削除 (チェック選択・カーソル単独の両方、Delete キー・右クリックの両方) は、「ごみ箱へ移すときは確認しない」(`skip_recycle_bin_delete_confirmation`) が ON でも**必ず確認を出す**。
- 確認文: 2 件以上でフォルダを含むときは、件数の行に「(うちフォルダ N 個)」を加える (例: 「12 件の項目をゴミ箱に移動しますか？（うちフォルダ 3 個）」)。既存の「フォルダの中には一覧に表示していないファイルも含まれます」は残す。1 件のときは今の文のまま。
- 実装: `delete_confirm_label_for_targets` が作る `DeleteConfirmContent` にフォルダの数を持たせ、`should_skip_delete_confirmation` はフォルダの数が 0 のときだけ省略を認める。判定は対象決定時の `GridItem` だけで行い、確認表示のためにディスクを読まない (今と同じ)。
- 対象決定時の種類はパスとともに `DeleteTarget` に保存し、メニュー待ち・確認待ち・承認後の worker 受付まで保持する。確認時や worker 開始時に現在の一覧から種類を判定し直さない。index は削除要求に保存せず、表示への完了反映は既存どおりパスで再解決する。
- コレクション root の登録解除・ブックマーク一覧の削除は対象外 (ファイルを消さない)。
- 保証の範囲は **mIV の削除** (Delete キーと mIV のメニューの「削除」) だけ。右クリックの「Windows のメニュー」にある Windows 標準の削除は、今も mIV の確認を通らず Windows の設定に従う (`native_context_menu.rs`、`docs/item-kind-capability-matrix.md` §6-15)。本件では変えない (設計レビュー P2 で指摘、Windows の動作に合わせる方針どおり)。
- 単独のフォルダを Delete したときも確認が出るようになる点は、設定 ON の利用者にとっての変化なのでリリースノートに書く。

### 3.2 本へ追加 (本書で決定)

- 今は 1 件でも追加できない項目があると全体を中止する。Ctrl+A がフォルダを含むようになると、サブフォルダのある場所では Ctrl+A → 本へ追加が必ず中止になる。
- 変更: **本そのもの (フォルダ・ZIP 本体・PDF 本体・変換前書庫) は対象外として飛ばし、残りのページを追加する**。飛ばした数をトーストに出す (例: 「「本名」へ追加中: 24 ページ (フォルダ・本 3 件は対象外)」)。残りが 0 件なら追加せず、「フォルダや本はそのままでは本へ追加できません。中の画像を選んでください」相当の案内を出す。
- ZIP・PDF・書庫もフォルダとそろえる理由: どれも「中にページを持つ入れもの」で、Ctrl+A で一緒に選ばれるのは同じ。フォルダだけ飛ばして ZIP で中止すると、同じ一覧で Ctrl+A → 本へ追加の結果が ZIP の有無で変わる。
- 動画・音声・同じ本のページなど、今「理由を出して中止」している項目は今のまま中止する (本件の対象外。動画の案内 `BOOK_GRID_VIDEO_HINT` は残す)。
- ファイル名スタックの展開経路は変えない。
- 判定は `book_page_source_for_idx` の前に種類で振り分ける (エラー文字列で判別しない)。メニュー・ツールバーの有効判定 (`ui_main.rs` の本へ追加の enable) が「対象外だけの選択」をどう扱うかも同じ規則に合わせる。
- 実機確認の結果による修正 (2026-11-11、サブ PC 担当が観測、依頼 32): 画像・フォルダ・ZIP を混ぜて本へ追加すると画像は追加されたが、「フォルダ・本 N 件は対象外」は見えず、「「名前なし」に 1 ページ追加しました」だけが出た。原因 (コード): 対象外の件数は追加開始時のトースト (`ui_fullscreen.rs` の「へ追加中」) にだけ載せていた。完了時の `book_append_refresh.rs` のトースト「に N ページ追加しました」が同じ欄を上書きする。サブ PC のログでは開始から完了まで約 0.05 秒 (`book append restore declines` 1081.401s → `book append: 名前なし added=1` 1081.449s) で、開始時のトーストはほぼ見えない。直し方: 対象外の件数を追加の依頼 (`BookOpIntent::Append`) に持たせ、完了時のトーストにも載せる。件数の持ち主は依頼 1 つにし、App に別の欄を足さない。
- 実装判断 (2026-11-10): 本体だけの選択でもメニュー・ツールバーは有効のままにして、実行時に中の画像を選ぶ案内を出す。対象決定は共通 helper に集約し、チェック済みを優先、なければカーソル単独へ fallback し、スマートフォルダのルールによる対象絞り込みも同じ規則で行う。

### 3.3 選択の合計サイズ (本書で決定)

- 今はフォルダの `image_metas` が一覧の種類で `None` (合計行が消える) か `Some((mtime, 0))` (0 として足す) になり、同じ選択でも一覧によって表示が変わる。Ctrl+A でフォルダが入ると、通常フォルダで今出ている合計行が消える場合がある。
- 変更: 合計は**フォルダ以外**の項目で計算する。フォルダを含むときは「合計サイズ X (フォルダを除く)」と出す。フォルダ以外の項目に未取得のサイズがあれば、今と同じく行を出さない。フォルダだけの選択では合計行を出さない。
- 「N 個選択」の数はフォルダを含めて数える (エクスプローラーと同じ)。

### 3.4 今のままにする操作 (確認済み)

- コピー / 切り取り / パスのコピー / D&D / フォルダへ移動・コピー: フォルダを中身ごと含む (エクスプローラーと同じ)。
- ★・タグ・コレクションへ追加: フォルダを含む (既存のコンテナ★・フォルダのタグ・フォルダのコレクション登録)。
- 外部ツール: §3.5 で変更する (2026-11-11 利用者決定)。
- ブックマーク一覧: 代理の `Folder` 行もチェックできるようにする。Ctrl+A → Delete で全ブックマーク行を消せるようになる (今は代理行が残る)。代理行に対する ★・コピーなどは、今カーソルでその行を選んだときと同じく元の本のフォルダに働く (チェックで新しく生まれる動きではない)。AI 説明文も同じく元の本の表紙が対象になる。
- AI 説明文: フォルダは本 (表紙 1 枚) として対象に残す。マニュアルの「対象は…本 (フォルダ・ZIP・…) の表紙」と一致し、選んだフォルダを黙って外すとかえって分かりにくい。処理は 1 フォルダにつき表紙 1 枚で、作成済みは飛ばし、進捗表示と中止がある。一覧の空白の右クリック (今のフォルダ全体) は今どおりサブフォルダを含めない。Ctrl+A の後に AI 説明文を作るとサブフォルダの表紙も作る点はリリースノートに書く。
- 一括書き出し・編集内容の貼り付け/リセット・補正スロット・回転・EPUB 変換・書庫の ZIP 変換: フォルダは対象外として飛ばす (一部は件数表示あり、一部は黙って)。対象外の数の出し方は今のまま。
- 名前の変更: 2 件以上は断る (今のまま)。

### 3.5 外部ツール (2026-11-11 利用者決定)

- 当初案 (2026-11-10) はフォルダの実パスも渡すものだった。Ctrl+A の後では件数が増え、複数選択の設定が「1 件」(`SelectionPolicy::Single`) のツールが画像 1 件 + フォルダの選択で起動しなくなる。利用者の判断で、**外部ツールへ渡すときだけフォルダを除き、今までどおりの件数で判定する**形に変えた。
- 規則 (一覧のチェックから対象を決めるとき = `ExternalTargetSource::GridContext` / `GridKey` で `checked` が空でないとき):
  - チェックした項目に**フォルダ以外が 1 つでもあれば、フォルダ (`GridItem::Folder`) を除いた項目だけ**を対象にする。件数・`Single` の判定・確認の閾値・上限はこの残りで行う。例: Ctrl+A の後でもファイルが 1 つなら「1 件」のツールが動く。
  - チェックした項目が**フォルダだけなら、今までどおりフォルダを渡す**。通常フォルダでは本件の前からフォルダを Ctrl+クリックでチェックして外部ツールへ渡せた。これを消すとリリース済みの動きの退行になるので残す。
  - 除いた件数は通知しない (渡す件数は今の確認ダイアログ・トーストに出る)。
- 変えないもの:
  - チェックがないときのカーソル / 右クリックした項目 1 件は今どおり。フォルダ 1 件を選んで外部ツールへ渡せる (v5.1.0 の修正「選んだフォルダを外部ツールへ渡せない問題」) は保つ。
  - `Container` (今のフォルダ・フォルダの上で右クリック)・`Viewer`・`Playback` は今どおり。フォルダ (や ZIP・PDF) の上で右クリックしたときは、チェックに関係なくその 1 件だけを渡す。v5.1.0 も同じ (`context_menu.rs` の `item.container_path().is_some()` の分岐)。マニュアル `external-tools.html` にも書いてある。2026-11-11 のサブ PC 確認 (依頼 32) で、フォルダ 2 つをチェックしてフォルダの上で右クリックすると右クリックした 1 件で起動したのはこの動きで、不具合ではない。確認手順の書き方 (「フォルダだけを 2 つチェックして同じツールを使うと件数で断られる」) が誤っていた。チェックしたフォルダだけを渡すのは、キー割り当て・ツール選択 (`GridKey`) と、フォルダ以外の項目 (チェックしていない画像など) の上での右クリック (`GridContext`) の 2 つの経路。チェック方式では右クリックでチェックが保たれるので、後者でもチェックしたフォルダが渡る (`ui_main.rs` の右クリック処理)。再確認の手順では、迷いのないキー割り当て・ツール選択を使う。
  - ZIP・PDF などのファイル本体はフォルダではないので除かない (今どおりファイルとして渡す)。
- 構造: 対象の決定 (`resolve_external_targets`) とメニュー・選択ダイアログの有効判定 (`external_tool_context_menu_descriptor` の capability) は、今はそれぞれ `checked` を見ている。フォルダの除外をどちらか片方に足すと、メニューでは有効なのに起動で対象が変わる、またはその逆が起きる。**チェックから対象の index を決める関数を 1 つにし、両方がそれを通す**。
- 状態の組み合わせを減らす検討: 除外は対象決定の時点で 1 回だけ行い、確認ダイアログ・準備中の処理には除外後の対象だけを渡す。準備中に一覧が更新された場合の扱いは今の `items_generation` の検査のまま (新しい状態を足さない)。

まれな失敗 (削除・移動の途中失敗など) は既存の経路のまま割り切る (ログ + 通知)。本件で新しい回復処理は足さない。

## 4. テスト

- 判定: `grid_item_can_be_checked` がフォルダで真になる一覧 (通常・レーティング・タグ・閲覧履歴・アイテム検索・名前検索・コレクション・スマートフォルダ・スナップショット・検索バー表示中) と、偽になるドライブ一覧。ZIP 内の `ZipDir` は偽のまま。
- 入口ごとの handler-level test (各一覧の代表としてレーティング一覧・コレクション root・ドライブ一覧):
  - Ctrl+A でフォルダが `checked` に入る / ドライブ一覧では入らない。
  - ゲームパッドの全チェックも同じ。
  - チェック方式の Ctrl+クリック・Shift+クリック範囲でフォルダが入る。
  - エクスプローラー方式の Ctrl+クリックでフォルダを足しても既存チェックが消えない (報告の症状)。
  - Shift+矢印の範囲・Space でフォルダが入る。
- 削除: フォルダを含むと設定 ON でも確認が出る / フォルダを含まない ごみ箱行きは今どおり省略される / 確認文に「うちフォルダ N 個」/ 1 件のフォルダは今の文。
- 本へ追加: フォルダ・ZIP・PDF・書庫が混ざっても残りのページで追加が始まり、対象外数が出る / 本だけの選択は追加しない / 動画が混ざると今どおり中止。
- 合計サイズ: フォルダ混在で「(フォルダを除く)」付きの合計 / フォルダの `Some((mtime, 0))` を足さない / フォルダだけで行なし。
- サブフォルダ展開: Ctrl+A 後の起点がチェックしたフォルダになる (仕様変更の固定)。
- コレクション: root の Delete は登録解除のまま (フォルダ会員も物理削除しない)。root の「元ファイルをゴミ箱へ」と、child (実フォルダの中) の Delete は物理削除 + フォルダの確認。
- ブックマーク一覧: 代理の `Folder` 行を含む Ctrl+A → Delete がブックマーク行だけを消す。
- 本へ追加: 本体を除いた後も、音声・同じ本のページが混ざれば今どおり中止する。カーソル単独の Stack の展開経路も今どおり。
- 合計サイズ: フォルダ混在でも、フォルダ以外にサイズ未取得があれば行を出さない。
- 外部ツール (§3.5): Ctrl+A でファイル 1 件 + フォルダが入った選択から、対象はファイル 1 件だけになり `Single` のツールが起動できる / `Each`・`Batch` の件数・確認の閾値・上限もフォルダを除いた数で判定される / メニュー・選択ダイアログの有効判定と実際の対象が一致する (フォルダ + ZIP ページなど、capability が変わる組み合わせを含む) / フォルダだけをチェックしたときは今どおりフォルダを渡す / チェックなしのフォルダ 1 件・`Container` は今どおり。
- 複数 context: main のフォルダのチェックが detached の開閉で変わらない。別 context のドライブ一覧ではフォルダをチェックできない。
- 既存の「フォルダは Ctrl+A の対象外」「展開不可の一覧ではフォルダを選べない」を固定しているテストは、仕様変更として直す (消すのではなく新しい仕様を固定する形に)。

## 5. 文書

- `docs/spec.md`: Space (2101 付近)・Ctrl+A (2115・3047 付近)・Delete の確認 (2116 付近)・`skip_recycle_bin_delete_confirmation` (2625 付近)・本へ追加・合計サイズの記述。
- `docs/keymap-spec.md`: `GridSelectAll` の説明。
- `docs/item-kind-capability-matrix.md` §6-6: Ctrl+A の食い違いを解消済みにする。
- マニュアル `htdocs/mimageviewer/manual/grid.html` (一覧のクリック選択の表の下にフォルダも選べることを 1 文、Ctrl+A の行)・`shortcuts.html` (Ctrl+A)・削除と本へ追加の説明があれば該当箇所。
- バックログ §1.732 を実装済みに更新し、下のリリースノート用メモを残す。

## 6. リリースノート用メモ (公開時に CHANGELOG / changelog.html へ)

- どの一覧 (★の一覧・コレクション・タグ・検索結果・閲覧履歴など) でも、フォルダを Ctrl+クリック・Shift+クリック・Space で複数選択できるようになった。Ctrl+A もフォルダを選ぶ。ドライブ一覧は対象外。
- 変わる動き (⚠️ として書く候補):
  - Ctrl+A はフォルダも選ぶ。ファイルだけを選ぶときは、種類の絞り込みでフォルダ以外の必要な種類を選んでから Ctrl+A を押すか、Ctrl+A の後に Ctrl+クリックでフォルダを外す。
  - 通常フォルダで Ctrl+A の後にサブフォルダ展開を押すと、選んだフォルダだけを展開する。
  - フォルダを含む削除は、「ごみ箱へ移すときは確認しない」が ON でも確認を出す。
  - Ctrl+A の後のコピー・移動・D&D・★・タグ・AI 説明文は、フォルダも対象になる。
  - 外部ツールは、ファイルとフォルダを一緒に選んだときはフォルダを除いて渡す。Ctrl+A の後でもファイルが 1 つなら、複数選択の設定が「1 件」のツールで開ける。フォルダだけを選んだときは今までどおりフォルダを渡す。
  - ブックマーク一覧で、ページが見つからないブックマークも Ctrl+A で選ばれる。
  - 本へ追加は、フォルダや ZIP・PDF などの本を飛ばして残りのページを追加するようになった (以前は全体を中止)。

### 種類フィルタのコード確認 (2026-11-10)

- **フォルダを隠せる**。`src/settings.rs` の `FacetItemKind::Folder` は「フォルダ」と表示され、`src/app/metadata_ops.rs` の `facet_kind_for_item` は `GridItem::Folder` をその種類へ対応付ける。
- `src/ui_main.rs` の `draw_facet_kind_menu` は選んだ種類を `facet_filter.kinds` に追加する包含式の絞り込み。`src/app.rs` の `passes_facet_filter` は集合が非空なら集合にない種類の行を除くため、フォルダ以外の必要な種類を選べば実フォルダが非表示になる。
- 種類を何も選ばない状態は「すべて」であり、フォルダを除外する操作ではない。そのため公開文書は「フォルダのチェックを外す」ではなく「フォルダ以外の必要な種類へ絞ってから Ctrl+A」と書く。
- この確認は一覧の種類フィルタの挙動であり、サブフォルダ展開の走査対象フィルタとは別。`ZipDir` も種類上は Folder だが、そもそも複数選択は対象外のまま。

## 7. 実装と検証の記録（2026-11-10）

設計方針からの逸脱なし。設定画面の旧説明もフォルダ必須確認へ合わせ、削除説明の既存の Enter 記述の不一致は現行 handler に合わせて修正した。製品のキー操作は変更していない。独立実装レビュー（GPT-6.1 Sol / xhigh）は追補も ACCEPT、未解決の指摘なし。

### 追加したテスト（19件）

`src/app/tests.rs` の `section1732_folder_selection_tests`:

- `folders_are_checkable_across_list_kinds_except_drives`
- `ctrl_a_and_gamepad_select_all_include_visible_folders_in_rating_and_collection`
- `shift_arrow_and_space_check_folders_in_rating_and_collection_but_not_drives`
- `ctrl_a_subfolder_expansion_roots_are_checked_folders_only`
- `collection_root_ctrl_a_delete_requests_unregistration_and_preserves_folder`
- `collection_physical_child_delete_keeps_folder_confirmation_with_skip_enabled`
- `ctrl_a_bookmark_delete_includes_missing_page_folder_proxy_without_deleting_files`
- `main_folder_checks_survive_detached_drive_context_creation_and_retirement`
- `ctrl_a_external_tool_count_includes_folder_and_preserves_container_target`

`src/ui_main.rs`:

- `section1732_selection_total_excludes_folders_with_or_without_metadata`
- `section1732_folder_only_selection_has_no_total_size`
- `section1732_folder_click_selection_preserves_checks_and_includes_shift_range`
- `section1732_collection_root_folder_clicks_are_checkable`

`src/ui_dialogs/context_menu.rs`:

- `section1732_collection_root_physical_delete_confirms_folders_with_skip_enabled`
- `section1732_mixed_delete_counts_folders_on_the_count_line`

`src/ui_fullscreen.rs`:

- `grid_book_add_selection_admission_keeps_container_guidance_and_checked_priority`
- `grid_book_add_skips_all_container_types_and_starts_remaining_page`
- `grid_book_add_container_only_selection_guides_without_starting_worker`
- `grid_book_add_container_skips_preserve_media_and_same_book_rejections`

### 変更した既存テスト

- `grid_item::tests::checkable_includes_real_files_except_folders`: 基礎判定は Folder を含まず、一覧の許可を受け取る純関数では選べることも検査。
- `folders_confirm_but_archive_containers_skip_when_setting_is_on`: 旧 `folders_and_archive_containers_skip_confirmation_when_setting_is_on` を新仕様へ変更。
- `delete_confirm_label_keeps_recycle_bin_wording_for_normal_targets` / `delete_confirm_label_warns_when_delete_may_be_permanent` / `delete_confirm_always_warns_for_folders_but_not_file_only_targets`: フォルダ有無の bool を件数に変更。
- `delete_target_kind_is_derived_from_the_existing_grid_item_without_io`: 単独 Folder の既存文言・件数・省略禁止も検査。
- `grid_book_add_rejects_mixed_audio_without_partial_copy`: 旧 `grid_book_add_rejects_mixed_invalid_items_without_partial_copy` の Folder を Audio に替え、全体中止の従来仕様を維持する試験へ変更。本体混在で続行する試験は上の新規試験が担当。
- `preferences_recycle_bin_delete_confirmation_snapshot`: 説明文に合わせ既存 PNG 1枚を更新。`preferences_folder_edit_restore_snapshot` も再生成したが PNG は byte 単位で不変。両方の更新指定なし比較も成功。

### 選択入口の監査

`rg -n '\.is_checkable\(' src crates tests` の残存は `src/grid_item.rs` の12箇所だけ。製品の呼び出しは `grid_item_checkable` 内の1箇所、残り11箇所は基礎判定の単体テスト。選択の入口には直接呼び出しがない。Ctrl+A・ゲームパッドは `grid_item_can_be_checked`、クリックの許可条件は `grid_folders_checkable` を使う。Space・Shift+カーソルの既存入口も同じ App 判定を使う。

### 検証のコマンドと結果

未コミットの本ブランチ差分、通常 feature、dev/test profile。nextest は 0.9.148、default profile、retry なし。nextest のビルドには `MSBUILDDISABLENODEREUSE=1` と `--build-jobs 1` を明示した。

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、2分40秒。既存警告あり |
| `cargo fmt`（引数なし、workspace 全体） / `cargo fmt --check` | exit 0 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険な文字 0件 |
| `python scripts/check_site_cards.py` | exit 0、53件中違反 0件 |
| 下記の focused filter を指定した nextest | 237件実行、236件成功・追加テスト1件の期待値誤りで失敗。`target/section1732/focused.log`、`focused-initial-junit.xml` |
| `cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(section1732_collection_root_folder_clicks_are_checkable)' --no-tests fail` | 最終修正後 exit 0、1件成功（Check/Explorer × Ctrl/Shift の4条件）。`target/section1732/collection-click-isolated-final.log`、`collection-click-isolated-final-junit.xml` |
| `UPDATE_SNAPSHOTS=1` と2件の設定 snapshot filter を指定した nextest | 2件成功。`target/section1732/snapshot-update-ready.log`。画像は実装担当と独立 reviewer が目視確認 |

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(section1732) | test(grid_book_add) | test(grid_click_selection_tests::) | test(grid_reclick_open_tests::) | test(selection_info_tests::) | test(delete_confirm_tests::) | test(grid_item::tests::) | test(folder_target_) | test(subfolder_expansion_toggle_uses_checked_folder_roots) | test(stale_aggregate_gamepad_item_actions) | test(stale_aggregate_ring_actions) | test(preferences_recycle_bin_delete_confirmation_snapshot) | test(preferences_folder_edit_restore_snapshot)' --no-tests fail
```

設定 snapshot の filter は `test(preferences_recycle_bin_delete_confirmation_snapshot) | test(preferences_folder_edit_restore_snapshot)`。更新指定を外した通常比較の2件も上記 focused 実行で成功した。

失敗した `section1732_collection_root_folder_clicks_are_checkable` は、Explorer方式の単独選択を `checked` に持つという試験側の誤解だった。既存 handler の `selected` / `checked` の表現に期待値を合わせ、Shift+クリックで Folder のチェックが必ず入る検査を追加した（旧フォルダ不可の実装では失敗する）。さらに Ctrl→Shift を同一セルへ連続送信すると既存の double-click pairing に入るため、各方式と各修飾キーを独立した harness の単独クリックへ分けた。2回目の再実行は選択の assertion をすべて通過し、最後の activation 数だけがこの入力列のため失敗した（`target/section1732/collection-click-final.log`）。いずれの試験修正も独立 reviewer が原因と検出力を確認済み。製品コード・待機処理とほかの236件は変更していないので、その成功結果は再利用する。

最終の単独再実行は1件成功。JUnitの集合を照合し、再利用した236件と合わせて **237件の異なるテストが成功**していることを確認した。追加した19件はすべてこの集合に含まれる。設定 snapshot 更新の2件を別の成功件数として重複加算しない。

初回の native 依存ビルドは TurboJPEG の並列 MSBuild で exit 1。ビルドジョブを直列にして準備を完了した。追加した一覧種別テストの `match` の型不一致も修正した後、libtest のコンパイルは成功（12分34秒）。続くテスト一覧の取得が `0xc0000135` で止まったため、依頼の手順どおり `vendor/ffmpeg/bin/*.dll` を `target/debug/deps/` へコピーして実行を再開した。これらの段階はテスト実行成功として数えない。

`test-lib-full.ps1` と `build-dev.ps1` は依頼どおり設計担当へ引き継ぎ、今回は実行していない。製品アプリの起動・実機確認・git commit/add/stash/reset/ブランチ切替も行っていない。

### 独立レビュー P1 対応: 削除対象の種類を要求時に保持

対象は実装コミット `ef18d7bd8` に対する独立レビューの指摘。初回実装は削除対象の index と確認時点の `items` からフォルダ数を求めていたため、外部再走査などによる一覧差し替えで Folder が別種類へ見え、確認省略設定 ON でフォルダの確認を省略する可能性があった。メニューを開いた時点とコマンド実行時点の間にも同じずれがあった。

- `DeleteTarget { path, is_folder }` を導入。Delete キーのカーソル単独・チェック選択、右クリック（ビューア・コレクションの物理削除を含む）の対象構築で、同じ `from_grid_item` から種類と実パスを確定する。確認文・件数・警告・省略判定は保存した対象列だけを使い、`delete_confirm_label_for_targets` は `items` を受け取らない。
- index の消費を調べ、削除要求には不要と確認して型から除いた。index 降順という既存の対象順は収集時だけ使って維持する。Shell クリップボードで使う既存の `collect_checked_indexed_paths` は変更せず、削除は `collect_checked_delete_targets` へ分けた。
- 独立追補レビューで、`start_delete_files` が現在の `items` から再び Folder を推測して `Tree / Exact` を決めていた消費端も確認。このままだと元 Folder が一覧から外れた場合に、配下書庫の閲覧位置削除やコレクションの子項目の失効に影響する。同じ対象情報を承認後まで保持し、`start_delete_files` も `Vec<DeleteTarget>` を受け取り、保存値から worker の範囲を作る形へ修正した。
- キャンセル・対象の置換・空要求・worker の終了／失敗経路は既存どおり。フォルダ判定の同期 I/O や新しい回復処理は追加しない。ユーザーが承認した物理削除・ブックマーク削除・コレクション登録解除の区別は変更しない。
- 独立実装レビュー（GPT-6.1 Sol / xhigh）は消費端の追補を含め ACCEPT、未解決の指摘なし。画面の文言・配置は変更していない。

追加した handler-level 回帰（`src/ui_dialogs/context_menu.rs`）:

- `section1732_delete_snapshot_cursor_folder_survives_list_replacement`
- `section1732_delete_snapshot_checked_folders_survive_list_replacement`
- `section1732_delete_snapshot_context_menu_survives_list_replacement`（単独・チェック選択の両条件）

削除要求／メニュー対象構築の後、旧 index がすべて画像を指す一覧へ差し替え、確認省略設定 ON で確認表示まで通す。元の名前・件数・フォルダ警告・worker 未開始を検査し、さらに確認承認の返却値も同じ対象・フォルダ数・`Tree / Exact` 範囲・パスを保持することを確認する。既存の対象列・確認状態遷移・直接 worker 受付のテストは新しい型へ更新した。

最終差分の自動検証（通常 feature、dev/test profile、nextest 0.9.148 / default、retry なし）:

| コマンド | 結果 |
| --- | --- |
| `cargo fmt`（引数なし、workspace 全体） / `cargo fmt --check` | exit 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、26.21秒。既存警告あり。`target/section1732-p1/check-final.log` |
| 下記 `test(section1732_delete_snapshot_)` の nextest | exit 0、追加3件すべて成功（0.913秒）。初回libtest再コンパイル9分36秒。`target/section1732-p1/regressions.log` / `regressions-junit.xml` |
| 下記関連 filter の nextest | exit 0、85件すべて成功（4.572秒）。`target/section1732-p1/related.log` / `related-junit.xml` |

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(section1732_delete_snapshot_)' --no-tests fail
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(delete_confirm_tests::) | test(section1732_folder_selection_tests::) | test(delete_key_) | test(request_delete_confirm_resets_cached_label_for_new_targets) | test(checked_file_operation_paths_include_real_files_and_folders_only) | test(unreadable_rename_journal_blocks_every_owned_mutation_before_worker_start) | test(xmp_hydration_cannot_restore_rating_after_normal_delete_purge) | test(delete_worker::tests::)' --no-tests fail
```

関連85件には追加3件を含むため、88件の異なるテストとは数えない。確認文・状態遷移・コレクション／ブックマーク境界・Shell共有対象・worker開始の拒否・XMP削除後の遅着・worker本体の回帰を含む。全件gateと確認用buildは従来の設計担当への引き継ぎのまま、本追補では実行していない。製品起動・git変更操作も行っていない。

### 追補: 外部ツールの混在チェックからフォルダだけを除く（2026-11-11）

§3.5 の利用者決定を実装。`checked_external_target_index` を対象 index 選別の共通関数とし、
`resolve_external_targets` と `external_tool_context_menu_descriptor` の `ordered_checked_indices`、
および起動前のフレーム分割準備を同じ関数へ通した。フォルダ以外があれば Folder だけを除き、
フォルダだけのチェックはそのまま渡す。Single・Each・Batch の件数、確認閾値、上限、
メニュー／ピッカーの capability は選別後の対象で一致する。ZIP／PDF 本体や仮想ページは除かない。
チェックなしの単独項目、Container・Viewer・Playback、除外数を通知しない方針も設計どおり。

コードで確認した前提と実装上の扱い:

- 対象を作る第三の経路は `advance_external_preparation_mounted` の Source 列挙だった。
  起動／ピッカー開始時に対象全件を同期 collect すると既存の応答性契約を壊すため、共通関数は
  既存 Enumerating の cursor 内で分類・列挙を進める形にした。各呼び出しは表示 entry 最大1件、
  既存の128 entry／2ms予算内で進む。新しい要求owner・非同期job・回復処理は追加しない。
- `rebuild_visible_indices` は非表示になった項目を checked から外す（WYSIWYG契約）。
  選別には現在の表示順を使い、index順や現在項目優先へ並べ替えない。全チェック項目に到達したら
  走査を終え、唯一のチェック項目が現在項目の場合の直接参照も維持する。
- 準備中の変更・終了・取消は既存の owner／items_generation／modal 契約のまま。
  確認・実体化workerには選別済み対象だけを渡す。UI同期I/Oや製品UI文字列の追加なし。

追加5件（`src/external_tool.rs`）:

- `external_tool_mixed_folder_picker_and_slot_single_deliver_only_the_file`
- `external_tool_mixed_folder_capability_matches_resolver_picker_and_preparation`
- `external_tool_mixed_folder_each_batch_threshold_and_limit_use_filtered_count`
- `external_tool_folder_only_checks_keep_order_single_folder_and_other_sources`
- `external_tool_folder_filter_preparation_keeps_large_classification_frame_budget`

変更2件:

- `src/app/tests.rs` の旧 `ctrl_a_external_tool_count_includes_folder_and_preserves_container_target` を
  `ctrl_a_external_tool_excludes_mixed_folders_and_preserves_container_target` に更新。
  実際のCtrl+A handlerで画像1件＋Folderをチェックし、Single／Each／Batchのslot受付、
  閾値・上限1件での受付、picker表示、Container維持を検査する。
- `folder_target_checked_order_limits_and_virtual_refusal_are_preserved` を
  `folder_target_mixed_checks_exclude_folders_and_preserve_virtual_refusal` に更新。
  Folder除外後のSingle受付とEach／Batchの実引数、ZipDir拒否を固定する。

追加のSingle試験はheadlessの実pickerボタンとslotの両入口を通し、準備要求からfake launcherへ
渡ったパスが画像1件だけであることを検査する。Each／Batch試験は実handlerの確認・上限判定まで
進める。capability試験は画像、ZIP／PDF本体、ZIP内画像、PDFページ、画像＋PDFページを含む。
大量試験は全フォルダ／末尾だけファイルの1024件と、4096件中先頭2件だけのチェックを含む。

対象経路のgrep監査:

```powershell
rg -n 'checked|ordered_checked_indices|resolve_external_targets|external_tool_context_menu_descriptor' src/external_tool.rs
rg -n 'external_tool.*checked|checked.*external_tool|ExternalTargetSource|resolve_external_targets' src --glob '*.rs'
rg -n 'external_tool' src --glob '*.rs' --glob '!external_tool.rs' --glob '!tests.rs'
```

製品のチェック由来の独立走査はresolver・menu descriptor・Source列挙の3経路で、すべて共通関数へ
集約した。右クリック側はdescriptorを作成・消費するだけ。pickerはSource列挙の結果を捕捉して
そのまま再利用する。設定previewは `LaunchTarget::from_grid_item` で現在項目1件を取り、checkedは
見ない。関連付けアプリの拡張子収集も一覧全体の走査で、チェック由来の外部ツール対象ではない。
それ以外に別の対象・件数・capability生成経路は見つからなかった。
監査出力は `target/section1732-external/target-path-audit.txt`。

マニュアルの詳細ページと一覧の右クリック説明、製品ページ、spec、capability表、keymap仕様、
外部ツール設計、backlog §1.732 のリリースノート用メモを更新した。過去の更新履歴は変更しない。
設計方針からの逸脱なし。全lib gateとbuild-devは今回も設計担当へ引き継ぐ。

最終差分の自動検証（HEAD `0c9f20d47` 上の未コミット差分、通常feature、dev/test profile、
nextest 0.9.148／default、retryなし、`MSBUILDDISABLENODEREUSE=1`、build-jobs 1）:

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、19.49秒。既存警告あり。`target/section1732-external/check-final.log` |
| 下記の焦点filterによるnextest | exit 0、7件すべて成功（0.562秒）。再コンパイル4分56秒。`focused-final.log`／`focused-final-junit.xml` |
| `cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(external_tool) \| test(section1732)' --no-tests fail` | exit 0、126件すべて成功（11.757秒）。`related-final.log`／`related-final-junit.xml` |
| `cargo fmt`（引数なし、workspace全体）／`cargo fmt --check` | exit 0 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険な文字0件 |
| `python scripts/check_site_cards.py` | exit 0、53件中違反0件 |
| `git diff --check` | exit 0 |

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(external_tool_mixed_folder) | test(external_tool_folder_only) | test(external_tool_folder_filter) | test(ctrl_a_external_tool_excludes_) | test(folder_target_mixed_checks)' --no-tests fail
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(external_tool) | test(section1732)' --no-tests fail
```

ログ・JUnitはすべて `target/section1732-external/` 内。JUnitの実行済み集合を照合し、焦点7件が
関連126件にすべて含まれることを確認した（133件とは数えない）。初回の関連実行は126件中123件が
成功し、旧件数の期待値が残っていた2件と、追加試験でworker専用のfile-list APIを直接使った1件が
失敗した。試験を新仕様と実際のpicker handler経路へ修正し、選別走査の早期終了も回帰で固定した後、
上記の焦点・関連を再実行して全件成功した。初回記録は `related.log`。

製品バイナリ起動・外部プロセスの実起動・実機確認・git commit/add/stash/reset/ブランチ切替は
行っていない。headless UIとfake launcherだけを使い、実workerを通す試験もlaunch authorizationを
与えない。依頼どおり `test-lib-full.ps1`／`build-dev.ps1` は実行していない。

### 独立レビュー P2・P3 対応（2026-11-11、d59ba2fe1 への追補）

独立レビューの ACCEPT WITH CHANGES の2件を修正した。

- P2: `external_tool_context_menu_descriptor` は現在の表示列を同期で走査せず、checked の
  index だけを列挙する。`idx_visible` で可視性を調べた候補列を既存の
  `ordered_checked_indices`／`checked_external_target_index` へ通す。capabilityは順序を使わないので、
  候補をHashSetの列挙順のまま渡してもよい。フォルダ除外の規則自体は引き続き共通関数が所有する。
- P3: 準備経路の唯一のchecked現在項目への近道にも `idx_visible` を条件として加えた。
  非表示のチェックを単独ターゲットへ読み替えず、resolver・capabilityと同じ空集合／対象なしにする。

表示集合についての調査・判断:

- 全表示項目への所属をO(1)で調べる既存集合は見つからなかった。既存の `App::idx_visible` は
  昇順の `visible_indices` に二分探索するO(log V)の判定。`keep_set` や表示近傍・settle計測用の集合は
  一覧全体を覆わず、`ViewerNavigationCaches` にも一覧の可視性を表す所属表はない。
- 新しいHashSet／bitsetは持たない。メニュー1回の計算量はO(C log V)、一時候補列の追加メモリは
  O(C)（C=チェック件数、V=可視件数）。全表示列のO(V)走査・複製は行わない。
  専用集合なら照合をO(C)にできるが、全可視件数分のメモリと、一覧採用・再構築・削除・snapshot・
  viewer context交換等での同期が必要になる。疎なチェックでは既存の二分探索で十分小さく、
  状態を二重管理する費用と不変条件を増やす必要がないと判断した。
- 通常の `rebuild_visible_indices_impl` はitemsをindex順に列挙して可視列を作り、最後にcheckedを
  その可視列へretainする。CollectionRootは全itemsを可視化するため追加のretainは不要。
  detached physicalの分岐も昇順の列を構築し、checkedをretainする。今回の判定はこのretainに
  正しさを依存せず、表示外チェックが残る合成入力も明示的に除く。
- `rebuild_details_order` は `visible_indices` のcloneをそのまま使うか、全indexから作った行を
  sortして戻す。rating順保持も可視集合との積集合＋不足indexの追加なので集合は一致する。
  resolver／準備だけが使う表示順と、capabilityが照合する昇順の可視集合は同じメンバーを持つ。

到達できない不正な表現を渡した場合の限界:

- 例: `visible_indices=[1,0]`（昇順という契約違反）や、Detailsで
  `visible_indices=[1]`／`details_order=[0]`（同じ長さだが別集合）を手動で作れば、
  二分探索によるcapability／単独近道と表示順resolverは一致しない場合がある。
  上記の通常builderはこの表現を作らず、同期処理の途中に外部ツールhandlerを呼ばない。
  これらの不変条件を毎回O(V)で検査する処理は追加しない。旧試験の逆順 `visible_indices` は
  製品と同じ昇順可視列＋Detailsの表示順へ修正し、従来の対象順の期待値を維持した。

追加テスト（`src/external_tool.rs`）:

- `external_tool_menu_sparse_checks_visit_only_checked_candidates`: 可視64件／10万件それぞれの末尾
  1件／4件をチェックし、Thumbnail／Details、GridContext／GridKeyで実descriptorを検査。
  実際のチェック候補照合＋共通選別のentry訪問は画像選択で2C+1、末尾1件なら常に3回。
  二分探索内部の比較回数はこのentry計数に含めない。計数は `cfg(test)` のthread-localだけで行い、
  本番バイナリにはcounterも分岐も残さない。
- `external_tool_hidden_checks_match_resolver_menu_and_preparation`: 報告の
  `items=[Folder(A),Image(B)]`／表示`[1]`／checked`{0}`／selected`0`を含め、空表示・
  非表示画像単独・非表示画像＋表示フォルダ・非表示フォルダ＋表示画像の5条件を、
  Thumbnail／Details、GridContext／GridKeyの計20条件で検査する。3経路の対象とcapabilityを比較し、
  対象なしの準備が選択なしのエラーになることも固定する。

変更ファイルは `src/external_tool.rs`（実装・回帰・既存fixtureの不変条件修正）、
`src/ui_dialogs/context_menu.rs`（既存の表示順fixtureのみ）、本書§7。製品UI文言・公開仕様の変更なし。
最終検証（HEAD `d59ba2fe1` 上の未コミット差分、通常feature、dev/test profile、nextest
0.9.148／default、retryなし、`MSBUILDDISABLENODEREUSE=1`）:

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、32.58秒。既存警告あり。`check.log` |
| 下記の焦点nextest | exit 0、追加2件すべて成功（0.981秒）、初回再コンパイル8分25秒。`focused.log`／`focused-junit.xml` |
| `cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(external_tool) \| test(section1732)' --no-tests fail` | exit 0、128件すべて成功（17.992秒）。`related.log`／`related-junit.xml` |
| 最後の `cargo fmt`（引数なし、workspace全体）／`cargo fmt --check` | exit 0、検証したRustソースのhashは不変。`format.log` |
| `git diff --check` | exit 0 |

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(external_tool_menu_sparse_checks_visit_only_checked_candidates) | test(external_tool_hidden_checks_match_resolver_menu_and_preparation)' --no-tests fail
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(external_tool) | test(section1732)' --no-tests fail
```

ログとJUnitは `target/section1732-review-fixes/`。実行済み集合を照合し、焦点2件は関連128件の
部分集合である（130件とは数えない）。製品UI文字列は変更していない。製品バイナリ起動・
外部プロセスの実起動・git commit/add/stash/reset/ブランチ切替は行っていない。
全lib gateとbuild-devは前回と同じく設計担当への引き継ぎとし、本追補では実行していない。

### 実機確認追補: 本への追加完了にも対象外件数を残す（2026-11-11、依頼32）

サブPC担当の観測は、画像・Folder・ZIP本体を混ぜた追加で画像は追加されたが、開始時の
「フォルダ・本 N 件は対象外」が見えず、完了通知だけが見えたというもの（2回確認）。
設計担当から提示されたログは開始1081.401s／完了1081.449s。この経過時間はサブPCの記録であり、
本追補の実機実測ではない。

修正前のコードで原因を確認した。`add_grid_selection_to_named_book` は対象外件数をローカルに数え、
`start_book_append_to_named` 呼び出し後の開始通知だけに付けていた。`finish_book_append_terminal` は
件数を受け取らず「に N ページ追加しました」を発行する。両方の `show_feedback_warning*` は
`publish_feedback_toast` を通り、同じ `fs_feedback_toast` へ代入するため、完了時に件数のある開始文を
上書きする。トーストの時間延長や遅延で回避せず、完了の事実と一緒に件数を通知する。

実装:

- `BookOpIntent::Append` に `skipped_containers: usize` を追加し、その追加依頼だけが件数を保持する。
  Appの別フィールド、完了summaryの重複状態、設定・永続化の追加はない。
- グリッドからの受付は既に数えた件数を `start_book_append_to_named` へ渡し、intentに保存する。
  開始文は変更しない。成功終端でintentから件数を読み、0件なら従来の文のまま、1件以上なら
  `「target」に 1 ページ追加しました (フォルダ・本 2 件は対象外)` の形で付ける。
  消しゴム補完・非表示ページ・Undo不可の既存の補足はそのまま後ろに続ける。
- 直接の呼び出し元を全検索し、グリッド以外の `start_book_append` ラッパーと画像fullscreen入口は
  0を渡す形にした。クリップボード・動画フレームもラッパー経由で0になる。ツールバーのピン留め本へ
  グリッド選択を追加する操作は同じグリッドhandlerなので、同じ選別と件数を使う。
- 失敗・receiver切断・Undoの通知経路は変更しない。件数の付加はAppend成功終端だけで行う。
  UI同期I/O、再試行・回復処理、既存機能の制限は追加していない。

所有と別contextの確認:

- 本操作はApp共通の単一 `BookOpPending` へ受理され、処理中の新しい追加は既存gateが拒否する。
  workerはその依頼専用のchannelに返し、`poll_book_op_pending` はそのintentを保持したまま処理する。
- metadata presence準備やtarget一覧refreshでreceiverが交代しても同じpending／intentを使うため、
  件数は失われない。件数を別の完了へ引き継ぐApp側の退避欄は作らない。
- 結果は既存 `source_context` の所有contextを一時mountして処理し、閉じたcontextの場合も従来の
  追加成功／失敗通知を処理する。`operation_id`／`source_context`／`accepted_revision` の照合と
  refreshの採用条件は変更不要。Undo資格の失効後も、実際に成功した自分の追加の件数は通知する。
  detachedの述語・viewport・context寿命の処理自体は変更していない。

追加したhandler-level回帰（`src/app/book_append_refresh.rs`）:

- `section1732_book_add_completion_reports_skipped_containers`: 画像1件＋Folder1件＋ZIP本体1件を
  チェックし、Ctrl+Bが呼ぶ `add_grid_selection_to_active_book` から実際のAppend workerと完了pollまで
  通す。開始文だけでなく最後の完了文が対象外2件を含むことと、追加した画像のbytesも検査する。
- `section1732_book_add_completion_without_skips_keeps_existing_message`: 画像1件を同じhandlerで追加し、
  intentの対象外数0件と、従来と完全一致する完了文を検査する。
- `section1732_book_append_refresh_retains_skipped_count_on_the_same_intent`: 同じ追加intentのまま
  readonly refreshへreceiverを交代し、完了通知まで対象外件数を維持することを検査する。
- `section1732_book_append_failure_and_disconnect_do_not_report_skipped_count`: 失敗とreceiver切断の
  両方で、対象外件数のない従来のエラー文を保つことを検査する。

既存の `section1373_detached_source_terminal_and_main_refresh_keep_separate_owners` と
`section1373_closed_source_preserves_success_and_newer_sibling_draft` にも、その追加依頼の対象外数を
完了通知へ載せるassertionを追加した。兄弟の一覧・draft・refreshの所有について既存の検査を維持する。
他のAppend intent fixtureには0を明示した。設計からの逸脱なし。

最終差分の自動検証（HEAD `58a53fa3d` 上の未コミット差分、通常feature、dev/test profile、
nextest 0.9.148／default、retryなし、`MSBUILDDISABLENODEREUSE=1`）:

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、27.50秒。既存警告あり。`check.log` |
| 下記の焦点nextest | exit 0、追加4件＋既存context回帰2件の計6件すべて成功（2.257秒）。初回再コンパイル11分13秒。`focused.log`／`focused-junit.xml` |
| `cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(section1732) \| test(book_add) \| test(book_append)' --no-tests fail` | exit 0、98件すべて成功（121.888秒）。既存のDB cold/warmベンチ1件が121.849秒でSLOW、失敗ではない。`related.log`／`related-junit.xml` |
| 最後の `cargo fmt`（引数なし、workspace全体） | exit 0、検証したRust8ファイルのhashは不変。`format.log` |
| `cargo fmt --check` | exit 0 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険な文字0件 |
| `git diff --check` | exit 0 |

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(section1732_book_add_completion_) | test(section1732_book_append_refresh_retains_) | test(section1732_book_append_failure_) | test(section1373_detached_source_terminal_and_main_refresh_keep_separate_owners) | test(section1373_closed_source_preserves_success_and_newer_sibling_draft)' --no-tests fail
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(section1732) | test(book_add) | test(book_append)' --no-tests fail
```

ログ・JUnitは `target/section1732-append-toast/`。JUnitの実行済み集合を照合し、焦点6件は
関連98件にすべて含まれることを確認した（104件とは数えない）。製品バイナリ起動・実機での
再確認・git commit/add/stash/reset/ブランチ切替は行っていない。全lib gateとbuild-devは前回同様、
設計担当への引き継ぎとし、本追補では実行していない。
