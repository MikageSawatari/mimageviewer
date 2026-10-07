# §1.346 詳細一覧の名前色 仕様提案

2026-10-08、ライン C。**§1.346 実装・自動検証・検証用ビルド完了。実機未確認**。
正本: [バックログ](next-release-backlog.md) §1.346。
関連: [UI snapshot方針](ui-snapshot-policy.md)、[仮想フォルダ](virtual-folders.md)、
[設定永続化](settings-sqlite-migration.md)。

## 決定済みとコード上の前提

利用者決定は「既定はフォルダとファイル系だけを控えめに色分け、ほかは利用者がカスタム指定」。
分類は **フォルダ／本 (ZIP・PDF・RAR・EPUB)／単体画像／RAW／動画／音声**。
サムネイル一覧へ色分けを追加する要望ではない。

実装前の `src/ui_main.rs` の `draw_details_row` は名前・他列・アイコンを共通色で描き、
`details_row_text_color` は選択時 `selection.stroke.color`、通常時テーマの primary を返す。
`details_row_background` は選択→チェック→hover→交互行→通常の順。
切り取り中は content painter 全体に `CUT_CONTENT_OPACITY=0.5` (`src/cut_clipboard.rs`) を適用する。
同じ描画関数は選択情報バー (display_only) でも使われるので、適用範囲を明示する必要がある。
`src/os_theme.rs::app_visuals` は標準／強い文字コントラストを持つ。
RAW は独立 GridItem ではなく Image 内にあり、`raw_format::is_raw_path` で分けられる。
名前欄の現在の描画は単色 text で、検索ヒット部分の着色はここにはない。
以下は利用者が決定した色の数値検証であり、製品上の視認性は実機未確認。

## 分類と適用範囲 (C346-2・3決定、利用者2026-10-08)

分類は I/O を行わない純粋な helper として既存 GridItem と保持済み source 種別から導く。
拡張子の独自重複表や代表サムネイルの種類で本体を分類する方式を避ける。

| カテゴリ | 決定した対象 |
| --- | --- |
| フォルダ | Folder、SearchContainer::Folder、ZipDir の非書庫ディレクトリ |
| 本 | ZipFile (CBZ含む)、PdfFile (PDF・EPUB。EPUBもこの型で一覧に入る)、ConvertibleArchive (RAR/CBR・他対応書庫)、ZipDir の書庫、SearchContainer::Zip |
| 単体画像 | Image の非RAW、ZipImage の非RAW、PdfPage、画像 Stack |
| RAW | Image／ZipImage の既存 RAW 判定。混在 Stack は画像分類、代表がRAWでも全体をRAWにはしない |
| 動画 | Video (再生モードにかかわらずファイル分類) |
| 音声 | Audio |

CollectionPlaceholder は「利用不能」の既存表現を優先し、六分類色を適用しない。
本を開いた内側ではコンテナ色で全ページを染めず、各ページを画像／RAWとして扱う。
変更するのは**詳細一覧の名前列のみ**。他列、プレビューアイコン、サムネイル一覧、
ツールチップ、選択情報バー、Remote Web UI への適用は追加しない。

## 決定した黄系の既定色とコントラスト

**C346-1 変更決定 (利用者2026-10-08):** 色分けは既定ON。フォルダだけ黄系で色分けする
(Windowsの黄色いフォルダを連想できる色)。他五分類は共通文字色を既定とする。
**C346-1a 決定 (利用者2026-10-08):** Light標準は `#A87E00`、Dark標準は `#D6BA66`、
Light強いは `#201800`、Dark強いは `#F4DFA2`。候補A一式の推奨はこの決定で置き換える。
Systemテーマは解決後のLight／Darkを使い、強い文字コントラストでは強い既定を使う。

計算法: sRGB の各成分を線形化 (c≤0.04045: c/12.92、それ以外: ((c+0.055)/1.055)^2.4)、
L=0.2126R+0.7152G+0.0722B、比=(明るいL+0.05)/(暗いL+0.05)。判定は丸め前の値、表は小数2桁。
背景はvendored eguiの `style.rs` と `src/ui_main.rs:3174` の `details_row_background` に基づく。
Light: 通常 `#F8F8F8`／交互 `#EFEFEF`／hover `#DCDCDC`／チェック `#A5A5A5`。
Dark: 通常 `#1B1B1B`／交互 `#282828`／hover `#464646`／チェック `#373737`。

標準／強いとも背景は同じ。純黄色 `#FFFF00` はLightで順に1.01／1.07／1.28／2.29:1となる。
利用者は黄系と認識できることを優先し、**標準フォルダ色の4.5:1基準を緩和**した。
Light標準の通常3.50:1、交互3.23:1、hover2.71:1は提示値を確認して受け入れ済み。
強い配色は4.5:1以上を維持する。チェック行ではカテゴリ色を使わないため、下表のチェック列は
共通primary文字色による**実際の表示比** (選択もされている場合は選択行の色・比を優先)。

| テーマ／文字コントラスト | フォルダHEX (通常／交互／hover) | 通常 | 交互 | hover | チェック共通HEX | チェック |
| --- | --- | ---: | ---: | ---: | --- | ---: |
| Light 標準 | `#A87E00` | 3.50 | 3.23 | 2.71 | `#373737` | 4.83 |
| Dark 標準 | `#D6BA66` | 9.08 | 7.78 | 4.98 | `#CDCDCD` | 7.49 |
| Light 強い | `#201800` | 16.59 | 15.32 | 12.85 | `#141414` | 7.48 |
| Dark 強い | `#F4DFA2` | 13.06 | 11.18 | 7.16 | `#F2F2F2` | 10.63 |

黄系の印象と実際の表示は実装時のsnapshotで確認する。全配色が4.5:1以上という旧案の主張は撤回する。

他五分類はLight標準 `#373737`、Dark標準 `#CDCDCD`、Light強い `#141414`、Dark強い `#F2F2F2`。
四背景でそれぞれ最小4.83／5.94／7.48／8.43:1となり、テーマ継承を維持する。

**選択・チェック行は全カテゴリで共通文字色、hoverだけならカテゴリ色 (C346-5変更決定、利用者2026-10-08)。**
`src/ui_main.rs:4981` の `details_row_text_color` はselected時に `selection.stroke.color` を返す。
Light `#00537D` / 背景 `#90D1FF` = **5.03:1**、Dark `#C0DEFF` / 背景 `#005C80` = **5.32:1**。
標準／強いとも成立し、選択＋hover／チェックでも選択背景・文字色が優先される。
チェックのみはテーマprimaryを使い、カテゴリの既定／カスタム色は適用しない。
Light標準の黄をチェック背景に使うと1.51:1となる。Dark標準のAは6.28:1で読みやすかったが、
テーマ別の例外を増やさず状態の組合せを減らすため、両テーマとも共通色へ揃える。
背景の優先順位は選択→チェック→hover→交互→通常のまま。チェック＋hoverも共通primaryとなる。
検索は既存のフィルタ／結果表示を変えず、新しい部分強調は追加しない。
将来名前の検索強調を載せる場合は該当部分の可読性を優先し、残りだけカテゴリ色にする。

**切り取りの決定 (C346-4、利用者2026-10-08):** 名前は不透明のまま、切り取りバッジと
プレビュー／他列の既存薄表示を保つ。名前の既存50%薄表示を変更する仕様は承認済み。
決定した標準フォルダ色を通常／交互／hover背景と50%合成すると最小比はLight **1.61:1**／Dark **2.41:1**、
選択文字はLight **2.08:1**／Dark **2.49:1** (sRGB各成分で丸める計算) となるため、名前には適用しない。

## 環境設定・保存・単純化の設計

環境設定の詳細一覧グループに「名前の色分け」チェック (既定ON) と六カテゴリの表を置く。
各行に「既定／カスタム」、Light色、Dark色、サンプル、カテゴリを既定へ戻す操作を用意する。
RGB不透明色のみ (alphaなし)、HEXと既存色pickerを使用。各テーマの背景上に通常・hover・チェック・
選択・切り取りのサンプルを出す。既定行は既定色を表示するが編集はカスタム選択後だけ。
全体を既定へ戻す操作も置き、設定OFFでも保存したカスタム色は捨てない。

**C346-6変更決定 (利用者2026-10-08):** カスタム色の入力・適用は標準配色だけ。
六カテゴリのLight／Darkごとに通常／交互／hoverの比を計算してサンプルと表示する。
**確定を止める条件は通常行の背景に対し3:1未満**。交互／hoverの低比だけでは確定を止めない。
いずれかが4.5:1未満なら「標準配色の名前色は4.5:1未満です。交互行やhoverでは読みにくくなる場合があります」
と該当状態・計算比を常時表示し、3:1未満の状態も明記する。通常行が3:1以上なら警告付きで確定できる。
通常行が3:1未満なら「通常行は3:1以上の色を指定してください」と表示して当該色の確定を止める。
警告のための追加確認ダイアログは設けない。チェック／選択は共通色なのでカスタム色の確定検査から除外し、
それぞれ実際に使う共通色のサンプル・比を表示する。既定のLight標準の低比もサンプルに明示する。
自動補正で指定値を変えたり、通常行で黙って無彩色へ戻したりしない。
強い配色ではカスタム入力・適用を提供せず、決定した強い既定色で4.5:1以上を維持する。
標準の保存カスタム値は保持し、標準へ戻すと再適用する。強い用の別カスタム欄は作らない。
選択・チェック時の共通文字色優先は常に説明する。

保存は一つの `DetailsNameColors` 設定 (enabled + 六カテゴリの Default / Custom{light,dark}) にまとめる。
既存 settings.db の加算的 serde default／Preferences draft→OK を使い、欠落は上記既定へ。
既存データを作り直さず、schema／専用DB／フォルダ別色／色cacheは追加しない。
環境設定移行用ファイルへの追加も既存方針に合わせ、利用者の指定色を設定OFFや通常↔portableで消さない。
表示状態ごとの保存色・一覧のlive rebuild・workerを増やす案は不採用。設定確定後の再描画で導出するだけ。
名前欄変更で新しいキー操作は追加せず、既存環境設定のキー／IME処理を使う。

## 利用者決定 (2026-10-08、未回答の利用者仕様質問なし)

- **C346-1 変更決定 (利用者2026-10-08):** 既定ON、フォルダだけ黄系、他五分類は共通文字色。
  青灰色を黄系へ変更し、標準フォルダ色は黄色の認識を優先して4.5:1基準を緩和する。
- **C346-2 決定 (利用者2026-10-08):** 推奨案を採用。仮想ディレクトリはフォルダ、本は全対応書庫、
  PDFページと混在Stackは画像、利用不能項目は既存表現とする。
- **C346-3 決定 (利用者2026-10-08):** 推奨案を採用。詳細一覧の名前列だけが対象。
  選択情報バーは従来色のまま、サムネイル一覧／Remoteへ広げない。
- **C346-4 決定 (利用者2026-10-08):** 推奨案を採用。切り取り中の名前は薄くせず、
  バッジとプレビュー／他列の薄表示で示す。
- **C346-5変更決定 (利用者2026-10-08):** 選択は共通の選択文字色、チェックは共通primaryを
  全カテゴリ・両テーマで使う。hoverのみなら分類色を保持する。
- **C346-6変更決定 (利用者2026-10-08):** 標準配色の六カテゴリのLight／Dark不透明色を指定可能。
  通常／交互／hoverの比を表示・警告し、通常背景に対し3:1未満だけ確定を止める。
  強い配色のカスタムは提供せず、標準のカスタム値は保持する。強い用の別カスタム欄は作らない。

**C346-1a 決定 (利用者2026-10-08):** Light標準 `#A87E00`、Dark標準 `#D6BA66`、
Light強い `#201800`、Dark強い `#F4DFA2` を採用。Light標準の通常3.50／交互3.23／hover2.71:1を受け入れ済み。
具体色の質問は回答済み。EPUBの型記述訂正も調整担当承認済み。未回答質問はない。

## 実装後の受け入れ・文書

分類の全variant／RAW・別名書庫・仮想ページ・Stack、旧設定欠落／custom保持／OKとCancel、
最終色の比と状態優先 (選択／チェック共通、hover分類色)、標準customの通常3:1確定境界・
交互／hover警告、強い固定色と標準customの保存保持をunit test化する。
Light／Dark×標準／強いのsnapshotに六分類、選択、hover、
チェック、切り取り、色設定表、長い日本語名を含め、snapshotは画像を目視する。
実装時はspec、一覧・環境設定マニュアル、製品ページと索引を更新する。

## 実装と検証記録 (2026-10-08)

`src/details_name_colors.rs` が分類・最終配色・状態優先・コントラスト計算を所有する。
`draw_details_row` の名前だけを不透明 painter へ分離し、他列とプレビューの切り取り表示を保つ。
環境設定は「表示→サムネイル」の既存 draft に表を追加し、既存 IME helper と RGB picker を使う。
未完成 HEX は編集中の egui 一時データ一つだけに置き、設定値は RGB だけを保持する。
フォーカス移動では失った欄自身の widget ID に一致する編集データだけを解放し、
同じフレームで別の欄が開始した未完成入力を取り消さない。新しい pending フィールドは追加しない。
新しい一覧状態・worker・ファイル I/O・キー操作は追加しない。
`details_name_colors` は既存 settings_kv に加算し、設定移行ファイルの分類・検査へ追加する。
旧設定の欠落時は既定値を導き、既存 DB を作り直さない。

自動検証結果は以下に記録する。製品の起動は行わない。
利用者はインストール版／tray常駐版を終了してから、当該worktreeで
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` を実行する。
single-instance mutex を共有する。通常の `%APPDATA%\mimageviewer` を使うため、
実際の設定・データを更新し得る。エージェントはこの実行を行わない。
実機では検証用ビルドで以下を確認する:

1. 詳細一覧でフォルダ・本・画像・RAW・動画・音声を表示し、Light／Dark × 標準／強いの最終色を確認する。
2. フォルダの hover は分類色、チェックと選択は共通色となることを確認する。
   Ctrl+X では名前が不透明、アイコン・他列が薄く、切り取りバッジが付くことを確認し、Ctrl+C で解除する。
3. 環境設定でカテゴリをカスタムにし、HEX／picker、各状態の比、通常3:1未満の OK 抑止、
   Light `#A87E00` の hover 警告付き確定、キャンセルとカテゴリ／全体リセットを確認する。
4. 色分け OFF と強い配色への切替後もカスタムが保存され、標準へ戻すと再適用されることを確認する。
   選択情報バー・サムネイル一覧・他列がカテゴリ色にならないことも確認する。

### EPUB の型記述訂正 (調整担当承認2026-10-08)

訂正前の分類表は EPUB を `ConvertibleArchive` に含めていたが、現在の
`src/app/folder_scan.rs` は EPUB を `GridItem::PdfFile` として列挙する
(`epub_is_paged_grid_item_and_same_name_pdf_wins_only_when_enabled` のテストも同じ型を検査)。
`src/archive_converter.rs::ArchiveFormat` には EPUB がない。
作成済みの分類 helper は両 variant を本とするため表示結果は一致するが、
「コードが設計前提と矛盾したら停止して報告」という実装指示に従い、後続の検証とビルドを停止した。

**調整担当決定 (2026-10-08):** 分類表を
「PdfFile (PDF・EPUB)、ConvertibleArchive (RAR/CBR・他対応書庫)」に訂正する。
EPUBは既存のPdfFile表現を使って本へ分類し、列挙・変換経路は変更しない。
この訂正を承認して残りの実装検証を再開する。利用者仕様の新規質問はない。

新規テストの DB 初期化は既存専用 `SettingsDb::open` からテスト専用 `create_new` に訂正した。
旧DBの新キー欠落も検査し、再開後の対象14件と切り取り描画1件は成功した。
検索索引の整合性検査は、新しい描画 helper の呼び出しと同 helper の表示文字列を確認する。
HEX欄のクリック移動＋同フレーム入力は、古い欄が新しい欄の未完成入力を消す red を確認し、
上記の widget 所有境界で修正した。red ログは `target/C-1346-hex-focus-red.log`。
### 最終自動検証 (未コミット差分、HEAD a3ff1cff5dd5b1c8cba10772d3fdffa82c44ac8f)

全コマンドは当該 worktree で実行。full lib は pipe なし、実終了コードを確認した。
入力所有境界と索引検査を修正した後の成功結果を採用し、失敗した初回結果では代替していない。

| コマンド | 結果 |
| --- | --- |
| `cargo fmt` / `cargo fmt --check` | exit 0。既存のCRLFを維持し、`git diff --numstat` に全体書換なし |
| `cargo test -p mimageviewer --lib name_color` | exit 0、15成功 (DB初期化・キー欠落・HEX移動を含む) |
| `cargo test -p mimageviewer --lib ui_dialogs::preferences` | exit 0、115成功。上記と重複あり、索引の整合性も検査 |
| `cargo test -p mimageviewer --lib details_name_cut_paint` | exit 0、1成功 |
| `cargo test -p mimageviewer --lib` | exit 0、10,952成功・52 ignore、994.15秒 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、通常feature |
| `cargo check -p mimageviewer --bin mimageviewer-core --features portable` | exit 0 |
| `cargo test -p mimageviewer --test ui_snapshot` | exit 0、111成功、更新フラグなしの比較 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険なglyph 0件 |
| `.\scripts\build-dev.ps1` | exit 0。normal featureのdev-runtimeでcore／Remote／EPUB workerを生成、DLLとVST3 bundleを配置。依存DLL検査はruntime=4／PE=3成功。起動していない |

snapshot は追加8枚 (1,133,075 bytes)、既存更新3枚を目視した。
切り取りの2枚は名前の不透明化、お気に入り設定の1枚は同じページの本文増加による
スクロールつまみの変化だけである。名前色のテストは各テーマ／配色を独立したharnessで検査する。
記録: `target/C-1346-focused.log`、`target/C-1346-full-lib.log`、`target/C-1346-ui-snapshot.log`、
`target/C-1346-build-dev.log`。コミットメッセージは `target/C-1346-msg.txt`。
`build-dist`／`test-full`／製品起動は実行していない。

検証用ビルドの初回は libjpeg-turbo の MSBuild が並列24で exit 1、並列2でも同じ境界で
停止した (詳細エラーなし)。当該worktreeの成果物に対する CMake の並列1は exit 0、
警告0・エラー0。スクリプトや製品コードを変更せず、`CARGO_BUILD_JOBS=1` と
`build-dev.ps1 -WaitForOtherBuildsMinutes 0` で再実行して exit 0。
core 15分43秒、Remote 3分46秒、EPUB worker 2分09秒。feature/profile は通常のまま。
