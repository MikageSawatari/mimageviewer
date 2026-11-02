# 本棚の本の並び順 — §1.683 改訂設計

作成: 2026-11-02。対象: v5。調査基点: `c728cd613`、worktree / branch: `mimageviewer-booksort-1683` / `booksort-1683`。
改訂: 2026-11-02。前案 `b48af3b9f` を同日の利用者決定に合わせて更新した。**§12 の利用者仕様・設計受入に基づき実装済み。独立実装レビューは修正後ACCEPT。自動検証・確認用buildの最終結果は§13に記録する。製品起動・commitは行わない（設計担当がcommitする）。**

## 1. 範囲と採用する仕様

「製本の管理」の本一覧を本棚ごとに並べ替え、その順を保存する。本の**ページ順**を変更する既存の「本の並べ替え」とは別の機能とする。

本棚直下の専用 JSON ファイルを正本とする。初期状態は従来の名前順。「製本の管理」の各行へ、お気に入りと同じ「↑」「↓」を置き、一操作ごとに表示へ反映して自動保存する。専用順序画面・一括「保存」・順序の「キャンセル」は設けない。手動順では既存本の相対順を保ち、未知の本は末尾へまとめて従来の名前順で並べる。番号・固定ボタン・追加先候補・前/次の本は、App 共通の表示 snapshot から導く。非同期保存中と未保存の表示は区別する（§5・§6）。利用者の決定は §12、既存 UI のコード調査は §2.5 に記す。

本内のファイル名、ページ番号、しおり、タグ、補正、読書位置、閲覧中の本を並べ替えだけで変えない。通常のフォルダ一覧として開いた本棚ルートのグリッドは既存のソート設定に従う。今回の「管理順」を通常グリッドの `SortOrder` や Ctrl+↑/↓ のツリー順へ混ぜない。

## 2. 確認した現状

### 2.1 リリース済み機能と互換性の前提

- `CHANGELOG.md` の v1.7.0 (2026-06-16) に「製本・本棚」を掲載。ローカル tag `v1.7.0` は `0c148b48e` で、その `src/books.rs` に `BookInfo` と本の保存処理がある。製本を未出荷データとして作り直すことはできない。
- tag `v4.11.0` は `2322f5a2c`。同 tag の `list_books` と `apply_append_to_cached_list` は調査基点と同じ比較規則。変更対象はリリース済みのフォルダ・名前順の解釈に対する追加仕様である。CHANGELOG とローカル tag を照合した証拠であり、今回は GitHub Release の公開状態をオンライン照会していない。
- [CLAUDE.md の永続データ規則](../CLAUDE.md)に従い、**旧データの読み替えを移行として必須**にする。順序保存だけが新規でも「移行不要」と扱わない。

### 2.2 一覧、番号、固定ボタンの実経路

| 箇所 | 現状と設計への含意 |
| --- | --- |
| `src/books.rs::list_books` | ルート直下のディレクトリだけを列挙し、各本のページ数を取得。`entry.file_name().to_string_lossy()` を名前に使い、ASCII 小文字化比較 → 元の名前比較で整列する。自然順・Explorer の照合順ではない |
| `books::apply_append_to_cached_list` | 既存行なら件数と path だけ更新して return。未登録行なら追加して名前順に再整列。本の追加のたびに無条件で全件ソートするわけではない |
| `App::request_book_list_refresh` / `poll_book_op_pending` / `complete_book_op_result` (`src/app.rs`) | `book-list` worker → `BookOpResult::List(Vec<BookInfo>)` → `book_list_cache`。一覧取得は `book_op_pending` と構造操作の排他を使う。現在の List 結果には root / revision が付かない |
| `draw_book_manager` (`src/ui_main.rs`) | キャッシュの順で列挙し、先頭20件に番号を出す。管理を開く操作と「更新」は worker へ再読込を要求する |
| `src/app/saved_group_actions.rs::book_action_target` | `GridOpenBook1..20` は rows の `slot - 1`。`GridBookPrev/Next` も同じ全件列で循環。未読込時は一度の入力を保持し、一覧確定後に対象を選ぶ。追加先を開く操作は名前で対象を捕捉する |
| 固定本棚ボタン (`ui_main.rs` の `TS::Bookshelf`) | **現在は `Settings.pinned_books` の格納順（固定した順）で描画**し、キャッシュは実在確認だけに使う。全件一覧と同順という backlog の含意は正確ではない。今回、表示 rows を固定集合で絞る順へ変更する必要がある |
| 製本メニュー・本棚ツールバーの追加先 ComboBox、ページ移動/コピー先 | 同じ `book_list_cache` を使用。並べ替えによって選択先を index で取り直してはいけない |
| `src/remote_ipc/collections.rs::bookshelf` | PC のキャッシュではなく `books::list_books` を直接使用。固定リストとして payload 化する。同じ resolver を使い、Remote 独自の再ソートを加えない |

### 2.3 キャッシュの更新経路と現行の不足

- 作成・本の改名・削除・ページの並べ替え・別本への移動/コピーの成功で `book_list_cache = None`。次の描画等で通常の一覧 worker を開始する。ページの並べ替え結果の `BookOpResult::Reordered` は**本内**の操作なので、本棚順保存用には別の名前を使う。
- 追加完了は `src/app/book_append_refresh.rs::finish_book_append_terminal` から `apply_append_to_cached_list` へマージする。固定ボタンが消えてツールバーが縮むのを防ぐ、リリース済みの経路である。追加 Undo は `src/app/book_append_undo.rs` で対象行の件数を減らし、順を変えない。
- 管理画面を開く、管理の「更新」、ページ並べ替え画面を開く、メニュー/ツールバー等でキャッシュがない場合にも一覧読込がある。外部のフォルダ変更を本棚キャッシュへ常時購読する専用 watcher は今回確認した経路にはなく、**管理を開き直す/更新する時に反映**する仕様を保つ。
- `src/ui_dialogs/preferences.rs::apply_preferences_settings` の `books_root_changed` は共有索引の同期だけを行う。本棚キャッシュの無効化や旧 List 結果の root 照合はない。旧本棚の rows が新しい本棚に見える候補経路であり、新しい順のキャッシュではこの所有不足を解消する。
- 一覧と製本操作は App 共通であり、`ViewerContextBundle` ごとに本棚一覧を複製する設計ではない。閲覧 context の items / thumbnail / channel / generation は別の所有物として維持する。
- 汎用改名は `ui_dialogs/rename_item.rs` の `rename_pending` → `shell_file_ops::rename_item_async`、汎用削除は `delete_pending`、整理 copy/move は `FileOrganizeRequest::Running` という別の owner。`admit_book_append_followup_mutation` は追加 Undo 中を拒否するだけで、これらと通常 Append の相互排他にはならない。全経路を順序 writer にする案は §6 で退け、対象を既存の製本操作へ限定する。

### 2.4 スクリーンショットの「mIV」が末尾だった件

報告された表示順は `genshin / LVM比較 / mychara / R18 / 他 / mIV`。実名がこの文字列そのものなら、現行比較の結果は **`genshin / LVM比較 / mIV / mychara / R18 / 他`** になる。二つの比較関数は同じであり、通常の List 採用・未登録行への Append マージだけから、報告順になる理由は見つからない。

`list_books` は実フォルダ名を読み、全角/半角、Unicode 正規化、trim、別名表示への変換をしない。一方、作成/改名/追加先から path を作る `normalize_book_name` → `sanitize_filename` は禁則文字を `_` に替え、**先頭/末尾の ASCII 空白と `.` を除去**する。全角英字や全角空白は半角化しない。この入口差はあるが、ASCII の先頭空白なら通常は前へ寄るため、それだけで末尾表示の説明にはならない。全角等は別の比較値になり得るものの、今回実名・文字コード・当該時点のキャッシュは取得していない。**原因は未確定**であり「全角だった」と診断しない。新機能と無関係な照合順変更はしない。

必要になった場合だけ、本棚一覧 worker の出力名と比較キーを escaped 表記で採取して実名と照合する。本設計を作るための追加操作を利用者へ求めない。

### 2.5 既存の並べ替え UI の調査（2026-11-02）

基点 `b48af3b9f` のソース調査。実画面を起動した観測ではない。`src/` の上下/左右ボタン、`swap` / move / reorder、drag の入力から反映・保存先まで追った。任意順を編集する画面に加え、指定されたブックマーク・リング・コレクション管理の「入替えなし」も区別して載せる。「専用キーなし」は並べ替えコマンド用のキー処理がない意味で、通常の widget フォーカス操作は残る。環境設定内の行は `PreferencesState.settings` の draft であり、表内の OK は共通の環境設定/操作カスタマイズの OK を指す（`preferences.rs::apply_preferences_dialog`）。

| 画面・対象（根拠） | 操作 | ボタンの位置・見た目・ラベル | 確定 | 選択の扱い | キー操作 |
| --- | --- | --- | --- | --- | --- |
| お気に入り (`favorites_editor.rs::show_favorites_editor_dialog`、操作列936行付近・反映1250行付近) | 隣接交換、drag なし | 各行右の「操作」列に通常ボタン `↑` `↓` `削除`、端は対応矢印無効 | 同じ frame 末で実 Settings を swap → `settings.save()`。下部は「閉じる」のみ | 行選択は不要。押した行が対象 | 専用キーなし。IME-safe Esc は閉じる（取消ではない） |
| スマートフォルダの定義 (`smart_folder_editor.rs::show_smart_folder_editor_dialog`、643/908行付近) | 選択項目を隣接交換 | 左一覧の上、「新規」の隣に通常 `↑` `↓` `削除` | 操作時に定義 draft を検証・確定し実 Settings を swap → save。閉じるだけでよい | ID による単一選択を維持 | 専用キーなし。IME-safe Esc/×も閉じる際の draft 確定対象 |
| 同・参照ルール (`smart_folder_editor.rs`、795/870行付近) | 行の隣接交換 | ルール見出しの右に small `↑` `↓` `削除`。端でも描画しクリックは no-op | その操作で定義を確定・save。名前検証に失敗したら確定しない | 行選択不要、選択中の定義内を編集 | 専用キーなし |
| 整理先フォルダ (`preferences/pages.rs::draw_file_organize_destinations_settings`) | 隣接交換、drag なし | 各登録の名前・パスの下、「参照…」の隣に通常 `↑` `↓` `削除`、端無効 | **OK**。説明も「OK で確定します」 | 行選択不要 | 専用キーなし、共通 OK/キャンセルの入力規則 |
| ピン留めタグ管理 (`tag_editor.rs::show_tag_editor_dialog`) | 隣接交換、drag なし | 各行末の操作列に通常 `↑` `↓` `削除`、端無効 | **OK** で draft を保存、キャンセル/×は破棄 | 行選択不要 | 専用並べ替えキーなし。IME-safe Enter=OK、Esc=取消 |
| 外部ツール (`preferences/pages.rs::page_external_tools`) | 選択項目の隣接交換 | 一覧下の共通操作行、通常「上へ」「下へ」（文字ラベル）、端無効 | OK | ツール UUID の選択を維持 | 専用キーなし |
| メニューバー上位/各メニュー項目 (`preferences/pages.rs::page_menu_layout`) | 同じ階層で隣接交換 | 上位行は見出しの左、項目行はラベル右に small `↑` `↓`、hover「上へ」「下へ」。端/固定位置は無効 | OK。「既定に戻す」も draft の変更 | 行選択不要 | 専用キーなし |
| 右クリックメニューの項目 (`preferences/pages.rs::draw_context_menu_layout_settings`) | 同じ階層で隣接交換 | 行右の固定幅 small `↑` `↓`、hover「上へ」「下へ」、アクセシブル名は項目名付き。端無効、1項目だけなら空欄 | OK | 行選択不要、階層を越えて動かさない | 専用キーなし |
| VST3 チェーン (`preferences/pages.rs::page_vst3` 内チェーン編集) | 隣接交換 | 行右に small `↑` `↓` と削除 `×`、hover「上へ」「下へ」、端無効 | OK でチェーン適用・保存 | 行選択不要。plugin の slot identity は別管理 | 専用キーなし |
| 同名画像の拡張子優先度 (`preferences/pages.rs::page_duplicate_files`) | 隣接交換 | 各行右、small `▲` `▼`。端は列を残して無効 | OK。「デフォルトに戻す」も draft | 行選択なし、scroll offset を維持 | 専用キーなし |
| ツールバーのセクション (`ui_main.rs::handle_toolbar_section_drag`) | 左 drag、6pt 未満は no-op | 見出し/ラベルが handle、挿入線。右クリック/設定内の「ドラッグで並べ替えを許可」が前提、OFF時はカーソル変更なし。上下ボタンなし | drop で実 Settings に反映・save | 項目選択不要、section ID を捕捉 | 専用キーなし |
| 詳細一覧の列/画像バーの列 (`ui_main.rs::interact_details_header_drag` / `finish_details_header_drag`) | ヘッダーの左 drag、閾値12pt | 列見出しと挿入位置表示、上下ボタンなし | drop で列設定へ反映・save | ファイルの選択と列順は別。描画面が所有する列だけ入替え | 専用キーなし |
| コレクション内の参照 (`collections.rs::draw_collection_reorder`) | 選択参照を `←` `→`、サムネ/選択件数handleの drag | 専用 Window 上部に通常左右ボタン（hover「左へ移動」「右へ移動」）と「閉じる」 | **閉じる/×で dirty を自動保存**、保存中は編集/close無効。保存/競合エラー時は保持し明示再読込/破棄（通常エラーは再試行も） | Ctrl toggle / Shift range、複数選択。entry ID で移動後も保持 | 専用入替えキーなし。Home/End/PageUp/PageDown はスクロール |
| コレクション自体の管理 (`collections.rs` の管理一覧、番号説明3886行付近) | 任意順の入替えなし | 番号は作成順、上下ボタンなし | 該当なし。参照の並べ替えとは別 | collection ID の選択 | 並べ替えキーなし |
| 製本した本の**ページ** (`ui_main.rs::draw_book_reorder`) | 選択ページを `←` `→`、サムネ/選択件数handleの drag | 専用 Window 上部に通常左右ボタン、hover「左へ移動」「右へ移動」、「閉じる」 | **閉じる/×で dirty を flush**（worker のファイル改名）。保存中は編集/close無効 | Ctrl / Shift 複数選択、path key で保持 | 専用入替えキーなし。Home/End/PageUp/PageDown はスクロール |
| 本の構成・元ページ (`ui_composition_editor.rs::source_list`、MovePageStep) | 選択ページを前/後へ | 元ページ一覧上部の通常「ページを前へ」「ページを後へ」、端無効 | editor の **保存**、キャンセルは破棄 | page ID で選択を維持 | この step の専用キーなし（editor の元に戻す等とは別） |
| 動画高画質化キュー (`ui_dialogs/video_upscale.rs::render_task_row` / `apply_video_upscale_task_action`) | queued の前/次の queued と交換 | 行右に通常 `↑` `↓`、hover「順序を上げる/下げる」。端クリックは queue 側で no-op、running は矢印なし | 一操作で queue 更新 → `save_video_upscale_queue()`、閉じるでの確定なし | task UUID が対象、行選択不要 | 専用キーなし |
| 補正レイヤー (`ui_adjustment_panel.rs::draw_local_adjust_layer_list` / `apply_local_adjust_panel_actions`) | 選択レイヤーを前後へ | 一覧下の共通操作行に通常 `↑` `↓` `複製` `削除`、端無効 | 即時反映、既存ページ編集の保存/Undo 境界へ渡す | 選択 index を移動先へ追従 | この入替えの専用キーなし |
| カラー化の制御点 (`ui_adjustment_panel.rs`、10087行付近) | 隣接交換 | 各点の色・強さの右に small `↑` `↓` `×`、端無効 | 値をその場で反映、呼出元の補正/レイヤー編集の保存境界に従う | 行選択なし | 専用キーなし |
| 注釈オブジェクト (`ui_text.rs::object_list_actions_ui`) | 選択物を前面/背面へ | 一覧外・下部固定操作行に通常 `↑` `↓`、hover「前面へ」「背面へ」、端無効 | 即時反映、同じ編集 pass の `save_comic_objects`。Undo 対象 | object ID の選択/複数選択を保持（上下は主選択1件） | この入替えの専用キーなし |
| ブックマーク（本/動画/音声のパネル、場所の全件一覧） | **任意順の入替えなし**。本はページ位置順、動画/音声は時刻順。全件一覧はソート選択 | 上下の順序編集ボタンなし。全件一覧はソート ComboBox | 並べ替え保存なし。全件ソートは即時再投影（`app.rs::set_bookmark_view_sort`、session 内の値） | パネルは現在位置を強調。全件ソート時は stable key で選択復元 | 順序入替えキーなし。追加/移動の既存 KeyAction は別操作 |
| リングショートカット/パッド方向リング (`preferences/pages.rs::ring_shortcut_context_editor_impl`) | **一覧の入替えなし**。固定方向 slot の割当を変更 | 各方向の右に操作 ComboBox、「既定に戻す」。上下移動ボタンなし | 操作カスタマイズの OK。preview は draft を表示 | direction slot 固定、行選択なし | 順序入替えキーなし。リング実行時の方向入力とは別 |

根拠ファイルはすべて `src/` 配下。ブックマークの順の正本は `book_bookmarks.rs::list_for_container` の `ORDER BY page_index_hint` と `video_bookmarks.rs` の `ORDER BY pts_secs` にも確認できる。

**利用者の記憶との照合:** 上下ボタンは管理一覧で最も広く使われ、お気に入りの即時反映・保存もコードと一致する。ただし「整理先も即時」「ブックマークも上下で任意順」は現コードとは一致しない（利用者がそれらまで断定したという意味ではない）。アプリ全体が一つの確定作法に統一済みではない。今回は製本管理に近い**お気に入りの各行 `↑` `↓` + 即時反映・保存**を基準とし、別画面での左右ドラッグ編集/閉じる時保存は採らない。

### 2.6 既存 UI の保存中・失敗・競合の実態

- お気に入りは実 Settings を交換してから同期 `Settings::save` → `settings_db::save_full`。UI thread 上で終わるため、その呼出し中の次クリックと保存 worker の競合はそもそもない。失敗は stderr/設定診断ログに残り、呼出元には成否を返さず、表示済み Settings を戻さない。並べ替え固有の警告・retry・外部ファイルとの比較はない。スマートフォルダ、ツールバーも同じ save 境界。動画キューもその場で atomic 保存し、失敗はログだけ。
- 環境設定は OK 時の draft 適用であり、操作カスタマイズ等には開いている間の live 値を保持する3-way 適用がある。コレクション参照は revision で競合を検出して編集を保持し、明示再読込/破棄を選ぶ。いずれも本棚の外部フォルダ変更を自動マージする既存機構ではない。
- 新機能へ既存の同期 I/O を持ち込まない。**UI の即時反映は合わせ、保存だけは worker へ**移す。連打は最新値へ集約する既存方針（[ui-responsiveness.md](ui-responsiveness.md) の latest-value writer）を使う。ファイル正本の保護と依頼されたログ+通知を加え、コレクションの retry/編集回復 UI までは移植しない。具体的な連打・失敗・外部競合は §5.2・§6・§8 を正本とする。

## 3. 保存先の比較と移行

| 候補 | 利点 | 費用・互換性 | 判断 |
| --- | --- | --- | --- |
| 本棚直下の専用ファイル | 本棚ごとの保存、ルートを丸ごと移動/複製しても追従。通常版/portable/異なる profile で同じ本棚順を読める。v4 の直下フォルダ列挙から除外される | 本棚への書込権限が必要。ファイルも一緒にバックアップする。複数の独立したアプリが同時に書く用途の共有編集は別仕様 | **採用（決定 2026-11-02）** |
| `settings.db` の Settings field / 専用 table | 設定バックアップに入る。本棚が読取専用でも保存可能 | root のキー管理と移動追従が必要。本棚をコピーしても付かない。未知 field と全体保存・版 gate の影響を受ける | 不採用 |
| data-dir の別 DB / JSON | Settings 全体保存と分離できる。本棚にファイルを置かない | root→順の対応、別 profile・別 PC・ルート移動時の扱いが増える。順だけ別バックアップが必要 | 不採用 |
| フォルダ名へ番号 prefix / 各本へ ID ファイル | 名前順で読む旧版にも順を反映できる / 外部改名を ID で追える | 本名・参照pathを変える、または全本への追加書込と ID 重複/コピー規則が必要。今回の順序保存には大きい | 不採用 |

### 3.1 保存形式（決定）

保存先は **`<本棚ルート>/mimageviewer.book-order.json`**、UTF-8（BOM なし）の JSON、形式 version は **1** とする。今回の `src/` 検索では同名を使う既存経路はない。隠し属性に依存させず、サブフォルダも作らない。temp は同じルートの `mimageviewer.book-order.<UUID>.tmp` という通常ファイルを `create_new` で作る。v4 に temp フォルダを「本」と認識させないためである。既存の同名ファイルが不正なら乗っ取らず、読取異常として保護する。

```json
{"version":1,"mode":"manual","books":["mychara","mIV","genshin"]}
```

- `mode` は `name` または `manual` の排他的な enum。`name` の `books` は空。ファイル欠落を `Name` に読み替え、最初の手動順の保存まで新規ファイルを作らない。空配列の `Manual` は空の本棚を手動順で運用する意味を持つ。
- エントリは本棚**直下のフォルダ名**。絶対 path・番号・ページ情報・固定状態は保存しない。実際の I/O に使う path は列挙済み `BookInfo.path` から取り、ファイル由来文字列を無検証で `root.join` して操作しない。
- 順序ファイルの名前を `normalize_book_name` に通さない。実名との完全一致を先に使い、大小文字だけ違う名前は既存 Windows path 比較規則で一意に照合できた場合だけ同一とする。空名、`.`、`..`、区切り文字、絶対 path、同一フォルダを二度指定する列を拒否する。曖昧な衝突・現在の lossy 名で区別不能なケースを勝手に統合しない。
- `version` 不明・不正 JSON・不正な列・読取失敗と、**ファイルが存在しないこと**を別に返す。読めない保存済みファイルを「保存なし」に変換しない。予約 path が directory/reparse point 等であれば安全に拒否し、外側への上書きを避ける。ファイル全体の上限は **16 MiB** とし、上限+1 byte までの bounded read で判定する。書込側も同じ上限を検査し、本数を黙って切らない。
- 利用者が「名前順に戻す」を押すと `mode:name, books:[]` を同じ即時保存経路で置換保存する。旧ファイルの削除で表現しない。設定 DB と既存本の形式・schema は変えない。
- 全バイトを書込 → `flush` / `sync_all` → handle を閉じる → 同一 filesystem 内で atomic replace とする。Windows は既存 `archive_converter::replace_file_atomic` の `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`、他 OS は `rename` と同じ契約にする。既存宛先がない初回も完成 temp だけを公開する。旧ファイルの先行削除、非原子的 copy による代替はしない。置換失敗時は旧原本を保持し、今回の temp だけ best-effort で除去する。停電時の媒体までの永続性は保証しない。

### 3.2 移行と旧版との往復

1. v4 の本棚（順序ファイルなし）を v5 で読むと、従来の ASCII 小文字比較 + 元名の tie-break による `Name` として表示する。追加/改名時も初回手動確定までは従来どおり名前順。読み込むだけの移行で本/設定へ書かない。
2. v5 で手動順を保存 → v4 で同じ本棚を開くと、v4 は順序ファイルを無視して従来の名前順で読む。v4 は直下の通常ファイルを本にしない。本内の画像保存・ページ並べ替えもこのルート直下ファイルを変更しない。
3. v5 に戻ると残っている順を再使用する。v4 で本の追加/削除/改名をした差は §4 の「外部変更」と同じ規則で解決する。旧版の追加・設定保存が順序ファイルを落とさないことを fixture で検証する。旧版にも手動順が見えるという保証はしない。
4. 本棚を丸ごと移すなら順序ファイルもコピーする。本フォルダだけをコピーした場合は未知の本として扱う。本棚フォルダの設定変更自体は本も順序も移さず、元へ戻せば元の順を読む。

**設定プロファイルの制約:** `settings_db::reject_newer_app_version` は新しい版で正常保存した DB を旧版の schema 更新前に拒否する。これは v4.11.0 にもある既存のデータ保護であり、専用ファイル案でも、v5 保存後の同一 settings DB を v4 が通常利用できる保証はない。今回保証するのは**本棚データの往復互換性**。実機で v4/v5 を使い分ける場合は各版に対応した別 data-dir から同じ本棚を指定する。既存 gate の解除や実設定の巻戻しを本件に含めない。同一 profile の往復起動は本件の保証範囲に含めない（§12 の決定）。

## 4. 本の増減・改名と不一致の規則

純粋 resolver に、保存ポリシーと今回正常に列挙できた実フォルダ列を渡す。`Manual` は「保存列で現存する本」→「列にない現存本を従来の名前順」の連結とする。番号はこの解決後の全件列に付け、欠落本の番号を空席として残さない。

| 出来事 | `Name` | `Manual` |
| --- | --- | --- |
| 管理から新しい本を作成 | 従来の名前順 | 既存の相対順を保って末尾に追加、同じ作成 worker の終端で順も保存 |
| 未作成の追加先へページを追加 / 別本へ移動・コピーして本ができる | 従来の名前順 | 作成と同じ。既存本へのページ追加では位置を変えない |
| 外部/v4 でフォルダ追加 | 次回の通常一覧読込で名前順 | 次回読込で末尾の未知グループへ。複数の未知本は名前順。読込だけではファイルを書かない |
| v5 の管理から改名 | 名前順で位置が変わる | 旧名の位置を新名に置換し保存。改名前に未知本だった場合も、その時点の解決列での位置を保つ。大小文字だけの変更も同じ |
| 管理から削除 | 残る本を名前順 | 成功した削除対象だけ列から除き保存。残る本の相対順は維持、以後の番号は詰まる |
| 外部/v4/汎用「名前の変更」で改名 | 新名で名前順 | 原則「旧名の削除 + 未知の新名の追加」。改名を推定して旧位置を自動移送しない。大小文字だけで一意に照合できる改名は同一本。位置を保つには「製本の管理」で改名する |
| 外部/v4/汎用操作で削除・追加・本を移動/コピー | 次回読込で名前順 | 削除された行は出さず、追加された本は未知末尾。読込だけでは保存列から削らない |
| 保存列だけにある本が再出現 | 名前順 | 次の成功した明示書込までは元の保存位置へ戻る。同名再作成と同じ本の復帰は区別できない |

未知グループは、書込せずに複数回読込すると、その時点の未知本の集合で名前順になる。例: 保存 `[C,A]`、外部追加 `D` → `[C,A,D]`、続いて外部追加 `B` → `[C,A,B,D]`。一度現れた未知本も必ず追加時系列で固定する、という仕様にはしない（§12 の決定）。次の手動順操作の保存、または製本の管理/製本 worker による本増減・改名の成功時に、その時点の現存全件列を保存して既知にする。即時順操作では表示を先に反映するが、未知本が保存済みになるのは書込成功時である。その時点で欠落している古いエントリは落ち、以後の同名出現は新規末尾となる。

外部変更に対して UUID・内容一致・NTFS file ID を導入しない。フォルダ単位で別の本棚へコピー/移動した本は移動先では未知本、コピー元では通常の削除規則とする。共有フォルダ上の複数プロセスによる同時編集・手動 JSON 編集の自動マージは保証しない。

ルートや本の読取不能を削除とみなさない。列挙/ページ数取得失敗では一覧全体を成功扱いせず、前回の確定済み一覧を維持して通知する。とくに `exists()==false` に I/O エラーを畳む現行実装を新しい移行判定へ流用しない。未作成本棚と、読めなくなった既存本棚も区別し、エラー時の空一覧を保存しない。

## 5. 操作と UI（決定 2026-11-02）

### 5.1 管理画面内の各行 `↑` / `↓`、即時反映・自動保存

既存の「製本の管理」の本一覧に直接追加する。各行右側の操作群に通常サイズの `↑` `↓` を隣接して置く（視覚順は `↑` → `↓`）。tooltip は「上へ」「下へ」、アクセシブル名は「本名を上へ/下へ」。先頭の上/末尾の下は列位置を保って無効化する。移動は隣接1冊との交換で、押した本が対象。お気に入り同様、**新しい選択行・複数選択・ドラッグは作らない**。名前変更欄、固定、追加先、開く等の既存操作を維持する。

クリックを受理した frame で新しい表示 snapshot を一度だけ採用し、番号・pin・候補も同じ列から再計算する。直ちに worker 保存を要求する。最初の有効な交換で `Name` → `Manual` となり、既知/未知を含む現存全件を保存対象にする。逆方向へ戻して名前順と同じ列になっても `Manual` のまま。移動後も同じ本の名前編集値・追加先・固定状態を維持し、widget ID は row index ではなく本の identity に結び付ける。フォーカスしていた移動ボタンも同じ本へ追従する。遠距離の移動を連打できるよう、**保存中でも上下ボタンは有効**とする。

一覧の見出しに現在の方式「名前順 / 手動順」を表示し、「今の順を固定」「名前順に戻す」を置く。前者は `Name` のときだけ有効で、0冊/1冊も現在の列を `Manual` として即時保存する。後者は `Manual` のときだけ有効で、押すと現存本の従来の名前順を即時表示し `Name` を保存する。確認ダイアログや別の保存操作は挟まない。名前順のまま押す no-op ではファイルを作らない。両操作も上下と同じ保存 owner・連打集約・失敗表示を使う。破損/未知版/読取失敗時はこれらを含む順序書込入口をすべて無効にする（§8）。

専用の「本の順序…」画面、「保存」「キャンセル」、閉じる際の保存確認は追加しない。manager の × は表示を閉じる操作であり、受理済み順操作を取り消さない。

### 5.2 即時反映と非同期保存の規則

| 場面 | 規則（既存作法との対応） |
| --- | --- |
| 連打/交互クリック | 毎回、現在表示している列へ1回ずつ操作を適用。単調な表示 revision を付ける。同時に書く worker は1件、後続は最新の完全 snapshot **1件**へ集約。時間 debounce やクリックの取りこぼしで調整しない |
| 保存中の次の順操作/名前順に戻す | 受理して即時表示。送信済み要求は書き替えず、完了後に最新 desired がその送信済み版と異なれば1件送る。古い成功結果で新しい表示を戻さない。保存前の列へ戻る操作も省略しない（`[A,B]→[B,A]` 書込中に `[A,B]` へ戻したら後者を保存する） |
| 保存表示 | 見出し付近に「保存中…」。成功した要求の mode+本identity順列が最新表示と一致すれば、その応答が古い revision でも最新まで保存済みとして終端する。不一致なら最新1件を続けて保存し、表示を消さない。クリックごとの成功 toast は出さない |
| 保存中の管理の× | 即座に閉じてよい。App 共通 owner が残り、受信口/最新要求/lease を捨てない。開き直しは保存中 snapshot を表示し、通常再読込を同時発行しない |
| 保存中の他操作 | 順序とは別の作成/追加/Undo/改名/削除/transfer/汎用 Shell 変更/本棚 root 切替/更新は終端まで新規受付しない。manager は既存の「処理中」表示と同じ理由表示を使い、勝手に予約して後から再生しない。既に走っている処理は順操作の開始前に終端させる。通常閲覧や既存 viewer を保存のために閉じない |
| 通常終了 | 受理済み最新要求の成功/失敗が終端するまで、既存の終了受付へ本棚 owner を接続し非同期に完了を待つ。UI 同期 join、終了時の新規保存、失敗後の完了待ち・再試行は作らない。OS強制終了は§8 |
| 書込失敗 | お気に入り同様に表示済み列は session 内に保持する。原本不変、ログ+一度の警告、「未保存」を表示。後続 desired は送らず batch 終端。自動再試行なし。次の明示順操作ならその時の表示全列を新しい要求として保存できる。「更新」/閉じて開き直す/再起動/次の通常一覧再構築はディスクの保存済み順を読み直す。順の記憶を新たな回復ファイルに保存しない |
| 外部変更の検出 | ファイル内容 token または直下フォルダ集合が基点から変わっていたら書かず、後続 desired も破棄。「本棚が変更されました。最新の内容を読み直します」と通知し、既存の一覧再読込へ戻す。自動マージ/自動再試行/編集を保持する競合解決画面は作らない。通常利用で起きるため保存前に必ず照合する |

上下の即時反映と連打の使い勝手はお気に入りに合わせる。同期 save を真似ず、一つの latest-value writer に限定する点、保存中/未保存の表示と外部照合は本棚ファイルを正本とするための追加契約である。

### 5.3 レイアウト・入力規約

- 既存 manager の `egui::Window` を使う。`default_pos` / `.open`、移動可、`anchor` 固定なし。見出し・方式/復帰ボタン・状態表示は一覧の ScrollArea 外。操作列が狭い場合は同じ行の操作群を折り返し、名前欄を画面外へ押し出さない。
- 縦バーは右端、`.auto_shrink([false, ...])`。本文高さは content rect と固定行の実寸から算出する。長名、0/1/21冊以上、小画面1093×614、高倍率でも最終行の矢印・×・名前順復帰へ到達できることを確認する。通常行に drag sense / Grab カーソルを追加しない。
- 背面 wheel/key/chrome は既存 `common_modal_dialog_open` / `gate_main_chrome` の規則に乗せる。manager を閉じた後も write batch の modal lease が必要な入力を所有し、worker state の有無だけで終端後まで入力を止めない。
- 新しいグローバル並べ替えキーや矢印キーの横取りはしない。ボタンの通常フォーカス操作を使い、名前欄の IME と既存 Enter/Esc helper を維持する。独自ショートカットは今回追加しない（[keymap-spec.md](keymap-spec.md) / [key-customization-impl-plan.md](key-customization-impl-plan.md)）。

## 6. 状態の組み合わせを減らす設計

先に [CLAUDE.md「設計の簡素化」](../CLAUDE.md) を適用する。前案の専用 modal editor は、利用者が既存 UI との統一と即時反映を選んだため撤回する。

| 検討した手 | 採否と理由 |
| --- | --- |
| 管理の各行に上下、同期 save | 操作は採用、同期 I/O は不採用。既存お気に入りの使い勝手と UI 応答性を両立する |
| 保存ごとに上下も無効化 | 不採用。ディスク速度で連打/遠距離移動を遅くし、統一の意図に合わない |
| 専用 editor / draft / 一括保存、close時保存 | 不採用。クリック後に別の確定操作を増やさない |
| 毎クリック独立 worker / 無制限 FIFO / 保存中に改名等も並行 | 不採用。書込逆転、古い本名、取消/rollback/resume の組合せを作らない |
| 一つの writer + 最新の後続1件、順序以外の変更だけ batch 中排他 | 採用。連打を即時反映しつつ、同時I/Oと変更組合せを除く。管理を閉じても owner は App に残す |
| 各consumer/別窓ごとの並べ替え | 不採用。App の共通 snapshot から番号・候補・固定ボタンを射影 |
| 全更新を cache=None → 再読込へ統一 | 一律適用は不採用。Append の固定ボタンちらつきを再導入する。件数差分マージを維持 |
| 本増減/改名/順序保存は共通 reader/resolver/採用へ接続 | 採用。同じ worker が完成 snapshot/token を返す。通常成功後に追加再読込を待たない |
| 外部 watcher の常時保存、汎用 Shell 操作も writer 化 | 不採用。外部は管理 open/更新で照合。汎用操作は未知末尾、管理改名だけ位置を保持（§12 決定） |
| viewer/window を閉じる・再ロードする | 不要。本棚順の変更で閲覧資源を作り直さない |

### 6.1 所有・即時表示・保存完了の契約

- `Option<Vec<BookInfo>>` を、**root identity・root選択epoch・表示 revision・保存ポリシー・解決済み rows・読取 token**を一体にした snapshot へ整理する。viewer items の generation と共用しない。表示 snapshot と最後に保存が成功した基点を区別し、方式だけでは読取異常を表現しない（読取異常時の名前順は保存可能な `Name` ではない）。
- read/write の受付と receiver は本棚の単一 owner に集約する。型付き phase は通常の読込/Ready、`Saving { 表示snapshot, 送信済み要求, 最新desired一件, 保存基点, lease }`、`Unsaved { 表示snapshot, 保存基点, 警告 }`、`ReadOnlyName { 名前順snapshot, 読取エラー }`、全件列挙失敗の`ReadFailed`相当とする。これは契約の例であり、既存 `book_op_pending` / `BookStructuralOp` と別の pending/bool 群を並立させない。保存済み基点と表示が同じかの判定は **mode と rows の両方**を見る。
- 最初の順操作は、Append/Undo/List/構造操作/未完了の本 open、`rename_pending`、`delete_pending`、`FileOrganizeRequest::Running` と未採用完了がないことを確認し、`SessionHandle::try_local_modal_lease` を取得してから受理・即時表示する。取得できなければ表示を変更せず理由を通知。batch は最新 desired の成功、書込失敗、競合のいずれかで終端し lease を解放する。管理の表示期間全体にわたる長期 lease は取らない。
- 一度に送信する書込は1件。UI は後続を最新値1件に置き換え、成功応答の request/root/epoch を検証してから次を送る。成功が表示 revision より古ければ**保存基点とファイル token だけを進める**。表示を戻さず、内容が異なる最新 desired を新 token に対して保存する。成功した送信済み要求と最新表示の mode+本identity順列が一致する場合は、最新表示 revision まで保存済みとして batch を終える（同内容を revision だけのために再送しない）。たとえば r1=[B,A] の書込中に r2=[A,B]→r3=[B,A] なら r1 成功で r3 も保存済みとする。一方 in-flight が最新と別内容なら、最後の保存済み列と最新が同じでも戻しの要求を落とさない。途中失敗/競合では未送信分を捨てて終端し、自動送信を続けない。
- 即時順序writeの worker は読込時に捕捉した「ファイル欠落/正確な内容 digest」と直下実フォルダ名集合を、各書込の前と atomic replace の直前に照合する。mtime だけで判定しない。自身の成功で token を更新して次要求の基点にする。外部の追加/削除/改名/順ファイル変更を見つけたら書かず通常 reload へ（§5.2）。独立した他プロセスとの最後の照合～replace間を完全排他にする共有編集は保証しない。自動マージ、OS全体の長期lock、journal は足さない。製本workerの順保守では、自身が成功させた作成/改名/削除のFS差分を反映した期待集合を照合基点とし、自操作による集合変化を外部競合にしない。別の外部差分を検出した場合はFSを巻き戻さず、順保守の警告として通常resolverへ返す。
- **List だけでなく作成/Append/Undo/改名/削除/transfer/順保存の全要求・全結果**へ受理時の root と root選択epoch を付ける。List にはさらに要求 revision を付ける。UI への rows/追加先/pin 投影は root/epoch が一致するときだけ。I/O なしの既存 path 比較を使い、UI で canonicalize / exists を呼ばない。
- 通常の作成/Append中の本棚設定変更は現行どおり可能。A で開始→Bへ変更→A完了なら FS/metadata/順保守は捕捉済み A で完了するが、B の rows/active/pin は変えない。元 viewer の正当な完了処理と App 本棚の採用資格を分離する。`A→B→A` でも旧epoch結果を採らず現在選択の A を通常読込する。**順序write batch中だけ**root変更を受け付けない（既存操作の一般規則へ拡張しない）。
- 順序 writer は既存製本 worker とこの即時順操作だけ。製本操作では FS/metadata と順保守を同じ worker 終端まで直列化する。後追い UI worker を別発行して間に操作を挟まない。`Unsaved` 後に本の構造変更をする場合は、session の未保存列を成功保存と混ぜず、ディスクの最後の正常ポリシーを worker が読み照合して処理する。正常な結果 snapshot で未保存表示を置き換える。
- Remote は既存の `list_books` + 共通 resolver を維持。保存batch中は local modal lease により Remote の新しいセッション取得を拒否し、既に Remote が所有/取得/排出中なら最初の順操作を受理しない。正常保存後は PC/Remote が同じ順を解決する。まれな書込失敗で PC が未保存列を保持中だけ、Remote の新しい読込は最後のディスク保存順となる。この限界を隠すための Remote 専用未保存ストアは作らない。
- 管理 open/close は表示寿命だけ。保存中の close/再openで receiver/desired を破棄しない。終端後の open/更新/別 root/通常終了で、未保存列を自動書込し直さない。保存中の通常終了だけは受理済み要求の終端を既存終了経路で非同期に待つ。古い List は root/epoch/要求revisionで拒否し、新しい表示順を上書きしない。
- 順序だけの変更は Append Undo の receipt/revision を無効化しない。汎用の「本の変異」intent へ雑に分類しない。viewer items/thumbnail/channel/cancel と本棚順序の owner は独立させる。

## 7. 更新処理の接続と退行防止

順の読込・解決・書込は `books` の本棚層（分割するなら `book_order` module）に集約する。UI 側へ JSON の解釈を散らさない。

| 既存の動き | 必要な接続と維持すること |
| --- | --- |
| `list_books` | worker でフォルダを列挙 → ポリシーを読む → 共通 resolver → snapshot。正常保存後のPC / Remoteに同じ順を返す（保存batch/未保存表示は§6.1）。互換用の生の名前順関数が必要なら命名で区別する |
| 既存本への製本追加 | `apply_append_to_cached_list` の件数差分と path 更新を残す。手動順の全件再ソート、cache の一時削除、ボタン消失を禁止する |
| Append による暗黙の新規本・作成 | FS 成功後、**同じ worker** で `Manual` の順保守まで行う。結果に順/新しい所属を載せ、UI は I/O なしで既存 snapshot へ採用。UI から保存 worker を後追い発行して、間に次の本操作を挟まない。`Name` は既存比較で新行を差し込む |
| Append Undo | 件数だけ減らす既存の経路を維持。空になっても本フォルダが残る現行動作を、順序項目の自動削除へ変えない |
| 追加先の本 | `active_book_name` と捕捉済み target path を維持。順が変わったから先頭本や同番号の別本へ切り替えない。作成で新本を追加先にする、削除時に既定名へ戻す現行動作は維持 |
| 固定 | `pinned_books` は固定集合として使い、描画順は表示 rows を絞る。ON/OFF で本棚全体の順を変えない。左クリック=開く、右クリック=その本へ追加、展開/折りたたみを維持。設定内の既存 vector を並べ直して保存する必要はない |
| 管理からの改名 | 既存の `BookStructuralOp` / key migration / bookmark journal を維持し、FS 成功後、同じ worker で旧名→新名の順保守を追加する。UI の追加先・pin 追従を残す |
| 管理からの削除・本内の transfer | 管理削除の成功対象だけ順から除き、失敗/取消で順を消さない。ページ移動/コピーでは本棚順を動かさず、移動先が新規にできた場合だけ末尾登録 |
| 汎用 Shell 改名/削除/copy/move | 順序 writer を追加しない。次回の管理 open/更新で外部変更と同じ照合をする。管理以外からの改名では位置保持を保証しないが、残る既知本の順は維持する。元来、追加先/pin の追従も製本管理の改名/削除の完了処理であり、汎用経路の意味を本件で拡張しない |
| 本内の並べ替え | `BookOpResult::Reordered` の metadata 移行・再読込を維持。本棚順序ファイルと本のページ順を混同しない |
| 更新、管理を開き直す | 最後の snapshot を保持し、成功した新 snapshot を採用（順序読取異常だけなら名前順+保護状態、§8）。再走査中の rows は busy として見せる。順が確定しない間に番号を旧 rows で先行解決しない |
| 本棚フォルダの変更 | §6.1 の root 所有・失効を `apply_preferences_settings` と設定復元の共通適用境界へ接続。通常表示中に本を勝手に移動/close/reload しない |

固定ボタンは従来の固定順から管理順へ変わる。これは依頼された「新しい並び順に従う」ための意図した仕様差であり、保存なしの `Name` でも固定本の部分列を名前順にする。固定した本の集合は移行・並べ替えのために削除しない。

## 8. 非同期 I/O とまれな失敗

[ui-responsiveness.md §4](ui-responsiveness.md#4-新機能追加時のチェックリスト)に従い、順序ファイルの open/read/parse、存在/種別/内容確認、フォルダ走査、ページ数取得、保存前照合、serialize、temp 書込/flush/sync/置換を worker に置く。UI はメモリ上の表示列の交換と最新要求の受渡し・snapshot 射影・非blocking receive だけを行う。大きい本棚は可視行描画とし、毎 frame の全件 clone/ソート/ファイル確認を増やさない。新しい decode・GPU upload は不要。worker 区間は perf 有効時だけ計装し、idle の周期 I/O/repaint を追加しない。原子的保存の手順は §3.1 を正本とする。

**まれな失敗はログ + 通知、保存済み原本を保持する方針で閉じる。** 自動再試行、黙った別保存先、独自 journal、未保存順の永続 pending、失敗回復のための完了待ちは足さない。利用者の順はキャッシュとして消したり再作成したりしない。

- 書込失敗: §5.2 のとおり PC の表示列は即時反映済みなので、batch を止めて「未保存」と警告する。最後の成功保存（連打の途中の版が成功していればその版）を消さない。初回なら順序ファイルは未作成のまま。更新/開き直し/再起動/**次の通常一覧再構築**では保存済みポリシーへ戻る。次の明示順操作による新要求以外から、失敗した列の自動保存をしない。独自の Undo/復旧画面は追加しない。
- 保存済みファイルの読取/parse/未知 version/上限超過/予約path異常: **warm/cold とも、正常に列挙できた本を従来の名前順で表示し、原本を上書きしない**。ログと「本の順序を読み込めないため名前順で表示しています」の通知、管理内の警告を出す。最後の正常 Manual を表示し続ける前案は撤回する。番号/前次/候補/pin はすべてこの名前順 snapshot にそろえ、本を開く/作成/改名等の既存機能は残す。順序書込（上下/今の順を固定/名前順復帰/本増減時の保守）だけを停止する。単純な再読込入口は既存「更新」/再open/再起動。読めるようになれば保存順を再採用する。ファイルの削除/隔離/初期化/空上書き/未知版のdown-convertはしない。
- ルート列挙/ページ数取得も失敗: 名前順の完全な列も作れないため、§4 に従って前回の一覧を維持して通知する。空一覧や部分列を削除として保存しない。順序ファイル異常だけの場合の名前順表示とは区別する。
- 作成/改名/削除の FS/metadata は成功し、順保守だけ失敗: FS を巻き戻さず成功と順保存警告を分ける。旧正常ポリシーと現存本を §4 で照合して表示する。改名した本が未知末尾になる限界を通知。旧順原本を保護し、読取異常なら前項の書込停止・名前順表示を使う。失敗した保守を後追い retry しない。
- 外部競合: §5.2 の通常 reload で最新ポリシーを読み直す。読み直し自体の失敗を回復する専用処理は作らず、前項の単純な通知/表示規則へ戻る。
- 強制終了: 最後に atomic replace が成功した完全ファイルだけを次回読む。未公開 temp を昇格しない。未送信/書込途中の操作までの保存は保証しない。残った temp は reader が無視する。過去の temp を自動で正本候補として選ばず、清掃/バックアップ/復旧機構を追加しない。

## 9. 番号、マニュアル、複数ウィンドウ

- 管理の先頭20件を「本1〜20を開く」へ割り当てる。21件目以降の「—」は現行どおり。固定本だけの通し番号へ変えない。未登録番号は現在の未登録通知を保つ。
- `GridBookPrev/Next` も管理順の全件で循環。追加先を開く操作は順と独立し、押下時の target を維持する。既存 Action ID、保存済み keymap、既定キーなしを変更しない。キー/リング等から同じ resolver を使う。
- 操作の有効な画面/context は既存仕様を維持する。Grid用の本番号操作を独立viewerへ新たに広げることはせず、複数窓の検証は共有本棚の整合と各閲覧contextの非干渉を確認する。
- 番号列 tooltip 案: 「操作カスタマイズの『本1〜20を開く』で使う番号です。本一覧の上から順に対応します。本の並べ替えや削除で番号が変わります。名前順では追加・名前変更でも変わります。」手動順でも削除で後ろの番号が詰まることを隠さない。固定ボタンも本一覧の順になる旨を近くに短く添える。
- 実装時に `htdocs/mimageviewer/manual/books.html`、`tut-books.html`、`shortcuts.html` の「本は名前順」を更新し、管理内の上下ボタン・即時反映/自動保存・初期名前順・未知末尾・番号変化を説明する。`settings.html` の本棚保存先説明とボタン説明、`index.html`、`privacy.html` の端末内保存の説明も照合する。本棚バックアップには専用ファイルも含むことを記す。今回は未実装機能を公開マニュアルへ先行記載しない。
- 実装時に `docs/keymap-spec.md` の「本は名前順」を「本棚の表示管理順（保存なし/順序読取異常は名前順）」へ変更し、`spec.md`、`architecture-overview.md` の保存先表、`async-architecture.md`、`ui-responsiveness.md` に owner/worker の契約を反映する。
- 複数ウィンドウでは App 共通の一つの本棚順を参照する。表示更新と保存結果は root/epoch で採用し、PC の consumer は同一表示 snapshot を射影する。Remote は §6.1 の保存境界に従う。開いている本の index/ページ順、parked/linked/independent の items、thumbnail cache、要求 channel、取消 token は変更しない。単なる本棚管理の open/close が兄弟 viewer の再読込や mutation invalidation を発行してはいけない。
- [detached-rework-plan.md §2](detached-rework-plan.md)を確認した。本棚順のための detached 専用フラグ/viewport 再生成/遅延は不要。共通受付の接続で detached 述語/viewport 自体の修正が必要と判明した場合は、実装前に ClaudeCode と独立 reviewer の構造合意を取り同計画 §11 へ記録する。

## 10. テスト・受入計画（実装時）

以下は設計時に定めた受入条件。実装と非対話検証の結果は§13を参照。実アプリでの検証は実施していない。

| 層 | 必須ケースと期待結果 |
| --- | --- |
| 純粋 resolver / validation | 保存なし→従来比較そのまま（ASCII case tie、数字文字列、和文、全角、空白）。手動の既存相対順/未知複数の名前順、欠落の飛越/再出現、大小文字だけの一意照合、重複/不正名/曖昧名の拒否。Name/空Manual/同じ見た目のManualを区別 |
| 一時本棚の保存/再読込 | 一操作の即時要求→atomic保存→新readerでも同じ順。読込だけではファイルを作らない。0/1冊の「今の順を固定」→新本は末尾。「名前順に戻す」→`mode:name`を置換保存→以後の新本は名前順。JSON UTF-8/version1/16MiB境界、tempが通常ファイル、既存宛先の原本先行削除なし |
| 互換 fixture | v1.7.0 出荷構造/v4.11.0 の本棚を無保存で読み替え。新順ファイルとtempは旧直下ディレクトリreaderの本に入らない。v5保存→旧版相当の追加/改名/削除/設定保存→v5で既知順+未知末尾。原本不変。旧readerはtagの実コードと照合し、現行resolverだけの往復で代用しない。v4/v5の別profileと既存newer-app-version gateも維持 |
| 本の lifecycle | 作成、暗黙作成、既存本追加（0件成功/重複skip含む）、Undo、本内ページ並べ替え、ページ移動/コピー、管理改名（未知本/大小文字だけも位置保持）、削除/取消/失敗。Name/Manual双方で位置/件数/pin/追加先の追従一致 |
| 外部/汎用操作 | `[C,A]+D→[C,A,D]+B→[C,A,B,D]`、外部/v4/汎用改名は未知末尾、欠落→再出現、次の成功保存後の同名再作成。汎用削除/copy/moveにwriterなし。root/本の読取不能を空/削除扱いしない |
| handler / headless UI | 各行↑↓を押したframeで表示/番号/候補/pinが更新、saveボタンや専用editorなし。端矢印は無効で位置固定。名前欄の値/IME/本identity/フォーカス/追加先が同じ本を指す。GridOpenBook1/20/範囲外/前次/読込待機とAction ID/割当を維持。pin集合`[A,C]`、全列`[C,B,A]`ならボタン`[C,A]` |
| latest writer（決定的channel） | 連打を毎回受理、書込同時1件/後続最新1件、成功順逆転なし。保存中の逆操作`[A,B]→[B,A]→[A,B]`を落とさない。古い成功応答は表示を戻さず基点tokenだけ進める。Manual→名前順復帰→再交換/固定もmode込みで最終値を保存。中間成功だけでは「保存中」を消さない。ただしr1→r2→r3で送信内容が最新表示と同一なら最新revisionまで保存済みとして終端し、保存待ちが残らない |
| 受付・close・終了 | 先行Append/Undo/List/構造/Shell操作と未消費完了があれば最初の順操作を拒否。Remote lease取得前は表示を変えない。write batch中の順操作は継続、別の本変更/root/更新は拒否。manager close/再open後も同じowner/最新要求を保持。通常終了は受理済み最新の成功/失敗終端まで非同期に待ち、失敗後retryなし。処理なしのopen/closeはI/O書込なし |
| 外部競合 / root所属 | 書込前と置換直前の順ファイル/フォルダ集合変更を拒否、外部原本不変・未送信desired停止・通知・通常reload。自動mergeなし。`A→B→A`の古いList/起点窓退役の応答を拒否。通常Create/Append中のroot変更は可能で、AのFS/順保守成功はBのcache/active/pinを変えない |
| 差分 / 複数窓 / Remote | 既存本Append/Undoで件数だけ更新し、固定ボタン消失なし。main/linked F12/independent/parkedのページ/items generation/cache/channel/cancel不変。本棚順でviewerをreloadしない。正常保存後PC/Remoteのresolver結果一致、Remote active中の最初の順操作拒否、local batch中の新規Remote取得拒否 |
| まれな失敗の代表例 | 初回/既存Manualの置換失敗で原本不変、PC表示のみ未保存・通知・後続自動送信なし。次の明示順操作/通常reloadの契約を検査。warm/coldの壊れたJSON/読取失敗/未知版/上限超過は**名前順表示 + 全順序writer停止**（上下/今の順を固定/復帰/構造操作の保守）、本の既存操作は継続。FS成功後の順保存失敗はFS成功+警告、旧列と照合。回復経路の多重障害行列は作らない |
| UI snapshot / geometry | 現行`book_manager_numbered_dark`更新、light追加。Name/Manual/保存中/未保存/読取異常、0/1/6/21冊/多数、長名、1093×614、倍率100%/200%。矢印の列と端無効、最終行/方式ボタン/×への到達、折返しとscrollbar重なりを矩形/scroll helperでも検査。ドラッグ用state/カーソルを新設していないことを検査 |

実装担当は狭い `scripts/test-focused.ps1` または明示filterで books / saved_group / book_append / structural と新しいwriter/handler試験を実行する。今回の実装依頼で指定された最終gateはfmt・UI glyph・`cargo check -p mimageviewer --lib --tests`・UI snapshot・`scripts/test-lib-full.ps1`を1回とし、失敗項目は単独3回確認する（設計時のworkspace全体gateから、利用者の最新指示に合わせて限定）。検証台帳にはsource/条件/結果を残し、独立reviewerは同じsourceの成功証拠を再利用して全件を重複実行しない。

製品修正完了時は `scripts/build-dev.ps1` で利用者確認用binaryを用意する。実機確認は管理内上下の連打/閉じ開き/再起動/名前順復帰/番号/pin/追加改名削除/root/複数窓の短いsuiteを計画し、所要時間・desktop入力・使い捨てデータを示して別途明示承認を得る。エージェント起動は `prepare-portable-smoke.ps1` の隔離portableのみ。今回の設計改訂は実行承認を含まず、通常profileの製品は起動しない。

## 11. 実装へ渡す単位と現時点の記録

設計改訂時の引継ぎ単位を以下に保持する。今回の実装依頼はこれらを一括で進める指示であり、実装結果は§13へ記録する。**本worktreeのcommitは設計担当が行う。**

1. **保存層と互換性**: `books`/新規`book_order`の純粋resolver、version1 JSON/16MiB/原子的置換、missingとUnreadableの区別、Name表示+書込停止、旧tag fixtureをまとめる。既存本/設定schemaを変えない。
2. **共通ownerと即時保存**: Appのroot/epoch付きsnapshot・単一read/write受付、表示revision・送信済み+最新1件のwriter、token/フォルダ集合照合、close再open/通常終了/leaseを接続する。連打・逆操作・失敗・外部競合を決定的channelで先に検証する。独立reviewはこの所有設計自体を確認する。
3. **lifecycleと全consumer**: 同じworkerの作成/暗黙作成/管理改名/削除/transferの順保守、Append/Undo差分、root切替、PC番号/pin/候補、Remote共通resolverを接続。汎用Shell操作にwriterは足さない。元viewerの非干渉と旧root完了を重点検証する。
4. **既存managerへUIを接続し文書更新**: 各行↑↓、見出しの方式/今の順を固定/名前順に戻す、保存中/未保存/読取異常の表示。headless handler/snapshot/geometry/IMEを検査し、§9の仕様・公開説明を同時更新する。専用editor/保存/キャンセルを実装しない。
5. **完成単位の独立review・最終gate・確認build**: §10を満たす証拠を揃えてClaudeCode受入へ渡す。実機確認は別の承認された検証枠で行い、今回実施済みとは記録しない。

段階はレビュー可能な実装順であり、部分的な機能を出荷する方針ではない。設計改訂時の変更は本書だけだった。今回は製品コード・テスト・関連文書も変更し、backlogの状態を検証結果と合わせて更新する。

### 11.1 前案の独立設計レビュー（履歴、2026-11-02）

`b48af3b9f`の前案には、別contextの`gpt-6.1-sol` / `xhigh`による「未解決P1/P2なし、推奨案の構造は受入可能」という回答があった。汎用Shellをwriterにしない、0/1冊Manual入口、全結果のroot選択epoch、読取異常時の原本上書き停止を反映した履歴である。**専用editor+一括保存・読取異常時に旧snapshot維持という旧契約の受入を、今回の即時保存・名前順fallback設計の受入へ流用しない。** 前回のUTF-8/リンク/backlog限定差分/diffチェック成功も当時の差分だけの証拠である。

### 11.2 今回の改訂記録（2026-11-02）

- 利用者の決定を§12に確定記録し、§2.5–2.6の既存UI調査を追加。§3・§5–8・§10–11を即時操作/自動保存へ更新した。
- 別contextの`gpt-6.1-sol` / `xhigh`がコードと改訂差分を確認。往復クリックで最新と送信済み内容が一致する際のrevision完了判定、製本worker自身のFS差分を反映する期待集合、根拠symbolの誤記を修正し、再確認で**未解決P1/P2なし、構造は受入可能**と回答した。ClaudeCodeの設計受入・実装/実機確認の代わりにはしない。
- 文書のUTF-8（BOM/置換文字なし）・末尾空白・§1–12の連続性・決定7項目・ローカルリンク先・JSON例・表の列数・本書だけの差分・`git diff --check`を確認し、すべて成功。製品コード編集、Rust build/test、確認用build、製品起動、commitは行っていない。

## 12. 利用者の決定（2026-11-02、未決事項なし）

1. **操作方法 — 決定 (2026-11-02)**: 「ほかの UI とそろえたい。ブックマークや整理先フォルダなどの UI を確認して、なるべく統一したい。基本は上下のボタンで入れ替えできる作りだったと思う」。§2.5の調査に基づき、お気に入りにそろえた**管理の各行 `↑` `↓`**を採用。行選択や専用順序画面・ドラッグは追加しない。
2. **確定方法 — 決定 (2026-11-02)**: 「お気に入りなどほかの UI と動きをそろえたい。お気に入りは即座に反映するタイプだったと思う」。コードでも一致。**一操作で即時反映し自動保存**、専用画面の一括保存は撤回。保存workerの直列化/連打/失敗は§5–8で確定。
3. **保存先 — 決定 (2026-11-02)**: **推奨A、本棚直下の専用ファイル**。`mimageviewer.book-order.json`、UTF-8 JSON version1、原子的置換。形式/書込/読取異常/旧版往復は§3・§8で確定。profile内の別保存先へ黙って切り替えない。
4. **既存環境と追加本 — 決定 (2026-11-02)**: **推奨どおり、初回は従来の名前順、最初の手動保存後は管理からの新規本を末尾、外部未知本は末尾グループ内の名前順**。読込だけで順ファイルを書かない。0/1冊は「今の順を固定」で同じ運用を開始可能。
5. **改名の位置保持 — 決定 (2026-11-02)**: **推奨どおり、「製本の管理」での改名は位置を保持。外部/v4/汎用改名は旧名削除+未知末尾**。既存の大小文字一意照合以外に安定ID/改名推定を追加しない。
6. **名前順への復帰 — 決定 (2026-11-02)**: **初版から「名前順に戻す」を用意**。利用者の確定方法の決定に合わせ、管理見出しのボタンを押して即時反映・自動保存。一括「保存」での確定という前案部分は撤回。確認画面なし、失敗も同じ規則。
7. **旧版往復 — 決定 (2026-11-02)**: **推奨どおり、本棚データの互換を保証し、v4/v5は別profileで同じ本棚を使う**。v4は手動順ファイルを無視して従来の名前順、v5へ戻れば保存順+未知末尾。既存settingsの新版保護gateを解除せず、同じsettings DBの往復は保証しない。
8. **固定ボタン — 決定 (2026-11-02)**: 「いつも管理の順にそろえる」。順序保存なし・保存済みNameでも管理の名前順、Manualは管理の手動順。既存利用者の固定登録順が変わることは了承済み。`pinned_books`は集合としてのみ使い、既存保存形式を変えない。


## 13. 実装記録（2026-11-02）

一覧・番号・固定・Remote・Append差分の経路は設計と一致。Windowsの管理改名は既存の宛先存在検査が大小文字だけの改名も拒否し、journalも同じ制約を持つ。§4の要求を満たすため、同じworker/journalの二段階改名へ接続し、元名から最終名のmetadata引継ぎを保つ。既存保存形式は変更しない。

§7・§12の固定表示は利用者の追加決定「いつも管理の順にそろえる」に従う。固定vectorは集合としてだけ扱う。

### 13.1 独立レビュー中の補正

- 保存中に管理を閉じると共通modalが通常menu/toolbarを止めるため、その期間だけ「保存中… / 管理を開く」の小さな再表示入口を描く。表示flagだけを戻し、通常再読込・保存ownerの再作成は行わない。
- rootの存在観測を`RootPresence::Missing/Existing`でsnapshot/tokenへ保持。再読込workerへ既知Existingを渡し、消失を空一覧として採らない。保存前と置換直前も照合し、既存空rootの消失を黙って再作成しない。
- coldのルート/ページ列挙失敗は`ReadFailed`で終端する。自動描画入口は再送せず、明示open/更新/番号入力だけが通常読込を一度開始できる。warmの前回一覧は保持する。周期的なI/O・通知を作らない。
- 通常終了はroot lifecycleへ独立したdeferred受付を接続し、受理済み最新要求が成功/失敗で終端してから継続する。既存の他ownerのtray規則を変更しない。
- 構造操作の完成snapshotで包む際も、本内transferのsource_entries採用を先に維持し、順保守の列挙失敗なら前回の完成一覧を残す。

小画面の管理Windowは実viewportから外寸を制約し、固定の本棚path/追加先名は1行省略＋全文hoverとする。本文の名前編集は維持する。仮想行は実button galleyとTextEditの行高・折返し行数から計算し、各行をphysical pixel、次にeguiのGUI gridへ切上げてslotを作る。行の描画前に`set_min_height`で実slotも予約し、自然高・隣接行step・末尾の到達性を検査する。これは空本文やclipを余白追加・判定緩和で隠さないための実装補正であり、行drag/新規shortcutは追加しない。

行内操作のIDを本identityへ固定し、上下操作では移動先の行を組込scroll要求で追う。仮想表示から外れる前に反映するため、このScrollAreaだけ`animated(false)`とする（組込`scroll_to_*`要求の補間にのみ作用し、wheel/drag入力の挙動は維持）。実Enter連続入力で追従を検査する。最初の保存で状態行が増えて下端のボタンを押し出す境界も実証できたため、短い保存/読込/未保存表示は既存操作行内に同寸法の枠を常時予約する。長い読取異常警告は別行を保つ。App側のpending scrollやoffset補正stateは追加しない。

最終の独立source再確認もACCEPT。既存のWindowDrag::Anywhereを維持し、root/owner・UIの未解決P1/P2なし。最新ASTの非対話実行結果は後段へ記録する。

### 13.2 担当・検証条件

実装は保存/lifecycle・App共通owner・UIにファイル所有を分け、別contextの`gpt-6.1-sol`/`xhigh`が設計と実装を独立確認した。§13.1の修正後ACCEPT、未解決P1/P2なし。レビュー担当はCargoを再実行せず、下記の実装担当の結果を利用する。実装依頼の指定に従い、最終本体lib全件gateは`test-lib-full.ps1`を1回、失敗項目は単独3回で確認する。製品は起動しない。

ビルド条件は`MSBUILDDISABLENODEREUSE=1`。初回cold checkのTurboJPEG CMake並列MSBuildがexit1となったため、同じ生成済みINSTALL.vcxprojを`/m:1 /nr:false`で構築し成功、後続checkは通常Cargoへ戻した。依存ソース・ビルド仕様の変更なし。

### 13.3 焦点検証の途中記録

- `cargo check -p mimageviewer --lib --tests`: exit0（`target/section1683-check-final.log`）。この後のUI調整・ReadFailed追加を含む最終sourceでは、後段のgateでも再確認する。
- UI検証用にCargoが生成したlibtest（`target/debug/deps/mimageviewer-93f10f9ef4ffb6a0.exe`）へ明示filterを渡し、独立な保存/owner経路を先行検証した。以後その製品経路は変更せず、UI配置だけを調整している。全件gateは最終sourceで別に行う。
- `books::`57件、`book_order::tests`21件、`app::book_append`39件、`app::book_structural_op`14件、`app::saved_group_actions`19件、Remote本棚1件、keymap本番号1件がすべてexit0。ログは`target/section1683-{books,storage,append,structural,number,remote,keymap}-precheck.log`。
- `app::book_shelf_tests`は13成功/1失敗。失敗は終了継続のheadless試験が未適用の`TexturesDelta`を捨てていたテスト側の不備で、既存lifecycle helperと同じ明示clearへ修正した。修正した当該1件は単独3回とも成功（`target/section1683-quit-{1,2,3}.log`）。製品の終了処理変更は不要だった。
- 旧版reader fixture、順序ファイル/通常tempの除外、実本棚の移動/全体コピー/本だけコピー、大小文字のみの管理改名は上記保存/本の試験に含む。旧版exeや旧設定writerそのものは起動していない。
- UI geometryは最小scroll高64による4pt overflow、狭い初期Windowによる本文0高、分数行高による最終矢印0.1ptクリップを検出し、宣言領域/実領域を直した。既定ppp1の全matrixは一度成功（15.35秒）。追加レビューで固定headerの長文制限、実ppp、仮想slotの実予約も補い、最終sourceで再検証中。判定の許容緩和は行わない。
- 状態表示の固定枠を含む完成sourceの管理UI回帰8件がすべて成功（11.50秒、`target/section1683-ui-lib-complete.log`）。実Enterの連続移動、初回保存時のviewport不変、分数ppp1.25/1.5/2と長名/深いroot、狭い表示での全操作到達性を含む。失敗履歴のある境界の単独再確認とPNG検査は別途記録する。

テスト用native graphでも並列24のCMakeが停止したため、`CARGO_BUILD_JOBS=1`で成功させた。以降の検証/buildも同じ条件とする。nextestの実行並列数やretry条件は変更しない。

### 13.4 完成sourceの非対話検証

対象は`booksort-1683` / `7a7b4a125`上の未commit差分。製品コード・test・fixtureは最終UI補正後に固定し、以下の検証中は変更していない。独立sourceレビューは修正後ACCEPT、未解決P1/P2なし。文書の結果追記は製品挙動を変えない。

| 検証 | 結果 |
| --- | --- |
| `cargo fmt` / `cargo fmt --check` | 成功 |
| `cargo check -p mimageviewer --lib --tests` | 成功、2分46秒。`target/section1683-check-complete.log` |
| UI glyph / site cards | 危険glyphなし、49 cards・超過0 |
| 管理UIのheadless回帰 | 8件成功、11.50秒。標準geometry100条件、実ppp+長名6条件、focus/close再表示/pinを含む |
| UIの失敗履歴4項目 | geometry/分数ppp/連続Enter/初回保存を各単独3回、12/12成功。`target/section1683-ui-alone3-summary.log` |
| 管理Window integration | 6件成功（5PNG更新＋12条件の×/方式ボタン到達性） |
| `cargo test --test ui_snapshot` | 更新modeを外した通常比較で224/224成功、49.21秒。`target/section1683-ui-snapshots-full.log` |
| PNG目視 | 代表5枚すべて確認、428,180 bytes。名前/寸法/QAは[UI snapshot方針](ui-snapshot-policy.md) |
| `scripts/test-lib-full.ps1` | 指定どおり1回だけ実行、exit1。15,534成功・7失敗・71 skip、1,382.192秒。成功gateとして扱わない |
| `scripts/build-dev.ps1 -PreserveRuntime` | exit0。通常機能・portable無効。core 53分48秒、Remote 3分13秒、EPUB worker 2分03秒。DLL/VST bundle配置とPE依存検査成功（runtime 4 / PE 3） |
| 差分・文字コード | `git diff --check`成功。変更/新規テキスト37件はUTF-8、BOM/NUL/新規の置換文字なし。既存indexのCSSコメント内の置換文字はHEADと同一で、本件では変更しない |

全件実行のsource識別子は`05c263586cad68dbfe17ade7fab78ed2d9c8317c60639eb5911118ce9f311c97`。command・stderr/stdout・JUnitは`target/test-lib-full/runs/20261102T122428803Z-089d7fdc/`。成功receiptは作られていない。workspace全体の`test-full.ps1`は最新の実装依頼の指定対象外として実行していない。

全件で失敗した7項目を、同じ製品source・feature/profile・期待値・時間制限のまま、選択だけ1件に限定して各3回実行し、**21/21成功**。既存`New-MivTestRun` / `Invoke-MivNextest`で通常profileを継承し、retryは0のまま。ログ/JUnitは`target/section1683-isolated/runs/`、各command/exit/秒数の索引は`target/section1683-isolated-summary.json`。

| 全件での失敗項目 | 単独再確認（秒、nextest summary） |
| --- | --- |
| `app::tests::history_parent_1473::section1473_other_synthetic_roots_parent_inventory` | 3/3成功、60.695 / 44.069 / 43.684 |
| `app::tests::smart_folder_transition_tests::no_resident_smart_pdf_missing_root_row_rolls_back_history` | 3/3成功、2.422 / 2.338 / 2.322 |
| `log_writer::tests::rotation_stall_and_snapshot_handle_preserve_saved_prefix` | 3/3成功、0.311 / 0.283 / 0.291 |
| `remote_ipc::raw_prefetch::tests::pending_retries_actual_admission_when_capacity_freed_before_pending_record` | 3/3成功、0.210 / 0.224 / 0.212 |
| `ui_adjustment_panel::local_adjust_segmentation_tests::full_mask_with_large_subtract_preview_completes_quickly` | 3/3成功、0.174 / 0.172 / 0.179 |
| `ui_fullscreen::page_turn::draw_submit_tests::page_turn_mainwindow_submits_visible_vacant_leaf_both_directions` | 3/3成功、2.973 / 3.135 / 3.067 |
| `app::folder_scan::section1375_scan_perf_tests::section1375_large_physical_scan_10k_100k` | 3/3成功、109.107 / 149.148 / 487.365 |

独立した限定source調査では、大規模走査はAppを生成せず新しい本棚ownerへ到達しない。Smartの欠落行検査も本棚read/writeを始めない。履歴inventoryはtoolbarから本棚readへ到達する接点があるが、全件失敗時のログは本棚case到達前のCollectionで終わっていた。これは到達関係の証拠であり、負荷などの原因確定ではない。大規模走査の600秒超過はfixture作成/片付けを含むtest全体であり、100kのscan処理自体が600秒かかったとは記録しない。単独で再現しなかった事実と全件実行の失敗を分け、期待値やtimeoutの緩和・新ignore・自動retry・無関係な製品修正は行わない。

確認用buildは単独再確認完了後に成功。成果物は`target/dev-runtime/mimageviewer-core.exe`（339,713,024 bytes）、`mimageviewer-remote.exe`（14,836,224 bytes）、`mimageviewer-epub-pdf.exe`（5,094,400 bytes）。command/条件/exitと各SHA-256は`target/section1683-verification-build.json`、script出力は`target/section1683-build-dev.log`、PE検査は`target/vcrt-pe-reports/dev-runtime.json`へ記録した。並列数切替のためのビルド停止は権限拒否で実行されず、最初の単一job実行がそのまま完了した。製品起動・実IME/実機操作・commitは未実施。

利用者がこのworktreeで確認する際の起動commandは`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。既定で通常の`%APPDATA%\mimageviewer`を使用し、実際の設定/データを更新しうる。single-instance mutexを共有するため、インストール済み/トレイ常駐のmImageViewerを先に終了する。管理内↑↓の連打、保存中の閉じ開き、番号/固定の一致、名前順復帰、再起動後の保存順を実機確認として設計担当/利用者へ引き継ぐ。

### 13.5 独立コードレビュー3件への修正（`e7bc35491`後）

設計担当から渡された独立Sol/xhighレビュー全文と該当sourceを照合し、3件とも正しい指摘と判断した。§13.1–13.4の当初レビュー/検証履歴は保持し、今回の追加指摘をそこで検出済みだったとは扱わない。利用者の追加指示に従い、今回は製品起動・commit・lib全件・build-devを行わない。

1. **LogicOnlyでの順保存終端**: `poll_book_op_pending`はFullの`render_other_worker_results`だけにあり、順保存のack・最新一件の後続送信・lease解放・終了再開がLogicOnlyで進まなかった。ShelfOrderだけを対象とするservice pollを`service_app_logic`へ接続し、root close受理の後で処理する。Full側はShelfOrderを重複pollせず、他の製本結果の表示採用を維持する。新しいqueue/pendingや待機は追加しない。`HeadlessEpiIntegration::update_logic_only`経由で、closeと同時の成功、失敗/切断、最新保存の成功/失敗、終了保留とlease解放、Listを消費しない境界を検査する。
2. **root切替後の番号入力**: `book_shelf_root_changed`が旧Listを保持し、新しい番号入力を旧Listのnavigationへ付けていた。共通設定適用境界で旧Listのreceiverを退役し、そのListに付いたidだけを取消す。Create/Append等のmutation workerと独立した追加先openは保持する。新しいrootは通常のListを開始し、A→B→Aでも新epochへ所属させる。旧返信前の新番号入力が現在の本棚で一覧→scan→採用まで完了するhandler回帰をA→B/ABA、旧番号待ち有無で検査する。
3. **既存本Appendの全本走査**: wrapperが追加前後の`read_snapshot`を毎回行い、常に`summary.shelf=Some`として件数差分経路を迂回していた。既存追加先は順の観測/保守を始めず、`shelf=None`で従来の件数差分を使う。新規追加先だけ直下名・policy/tokenの軽量観測を行い、同じworker内の作成後保守と完成snapshotへ接続する。resolverは名前列とBookInfoで共用し、仮のpage_count行を作らない。cache未登録の既存本は表示policyで所属だけ解決し、Manual/Unsaved/ReadOnlyの順・token・phaseと大小文字aliasの本identityを保つ。未登録本の件数が追加差分だけになる既存の限界を、全本走査で拡張しない。thread-local RAII recorderでページ列挙境界を捕捉し、公開/runtime APIの既存Appendが他本へ到達しないこと、新規Appendの他本列挙が作成後の一回だけであることを検査する。

今回も別contextのSol/xhighが設計・完成sourceを独立確認し、3件とも修正後ACCEPT、未解決P1/P2なし。実装担当が下記検証を所有し、reviewerは同じ証拠を利用する。検証条件は`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`、製品source/test固定。

| 今回の検証 | 結果・証拠 |
| --- | --- |
| `cargo fmt` | 成功 |
| `cargo check -p mimageviewer --lib --tests` | exit0、4分40秒。`target/section1683-reviewfix-check/{stdout,stderr}.log` |
| 焦点nextest | 198/198成功、実行19.435秒（compile13分01秒）。`target/section1683-reviewfix-focused/runs/20261102T143656720Z-1a0fdb01/`にcommand/config・stdout/stderr・JUnit |
| 新規回帰 | 13testすべて上記198件に含まれる。LogicOnly 4、mutation保持1、root切替番号/独立open 3、Append API/差分5。各test内の成功/失敗・root往復・policy・公開/runtime条件もすべて成功 |
| `cargo test --test ui_snapshot` | UPDATE_SNAPSHOTSを外し224/224成功、実行40.74秒（compile2分54秒）。`target/section1683-reviewfix-ui-snapshot/{stdout,stderr}.log`。PNG変更なし |
| UI glyph | 危険glyphなし |

焦点の選択はbooks / book_order / app::book_shelf_tests / saved_group_actions / book_append系 / book_structural_op / root_lifecycle / egui037_connection_tests / Remote本棚 / keymap本番号。全件実行への拡大、失敗のretry、期待値・timeoutの緩和は行っていない。製品起動・commit・lib全件・build-devは依頼どおり未実施。§13.4の確認用バイナリは今回の修正を含まない。
