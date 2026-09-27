# EPUB → PDF 変換の本体組み込み設計 (2026-09-25、第 8 版 = 独立レビュー 8 回目で実装開始可、持ち越し 4 件を反映)

- 経緯と検証: `docs/backlog-on-hold.md` §1.247 (計画)、`docs/epub-pdf-spike-results.md` (スパイク結果)。
- 変換器: `crates/epub-pdf-worker` (単体 CLI `mimageviewer-epub-pdf`)。
- 状態: **設計合意済み (独立レビュー 8 回目で「実装開始可能」、2026-09-25)**。持ち越しの修正 4 件は反映済み (§8 第 8 回)。各回の指摘と対応は §8。
- 行番号は `epub-pdf` ブランチ (master `a887ba43a` 起点) のもの。

## 1. 決定事項 (利用者合意 2026-09-25)

| # | 決定 |
| --- | --- |
| D1 | DRM のない EPUB を固定レイアウトの PDF へ変換し、既存の PDF 経路で読む。汎用 EPUB ビューアにはしない |
| D2 | 変換結果は APPDATA 配下の専用キャッシュに置く (案 A。実装上は RAR/7z の `archive_cache` とは分けた `epub_cache`、§4.4)。元ファイルの隣には書かない。NAS / 読み取り専用メディアでも動くこと |
| D3 | ページ単位・本単位の利用者データ (補正・回転・レーティング・タグ・注釈・消しゴム/モザイク・読書位置・しおり・履歴・コレクション・代表サムネ・見開き設定等) は**元の EPUB のパス**をキーにする。キャッシュの削除・置き場所に影響されない |
| D4 | EPUB 由来の PDF は容量上限による自動削除の対象外 (合計にも数えない)。キャッシュ管理画面の明示操作でのみ消す |
| D5 | 同名の PDF がある場合は EPUB を一覧から隠す設定を追加し、既定 ON (`skip_archive_if_zip_exists` と同型) |
| D6 | リフロー型は電子書籍端末相当の**固定**ページ寸法で組む。設定は作らない。本の CSS は上書きしない |
| D7 | 朗読音声・動画は PDF に入らない。マニュアルに明記する |
| D8 | WebView2 の束縛は `webview2-com` (Loader 静的リンク、DLL 同梱不要) |
| D9 | 一度読み込んだ本は、実行中に元の EPUB を差し替えても次回起動まで差し替え前を表示する (I7) |
| D10 | 綴じ方向の自動適用は設定「PDF / EPUB の右開き指定に従う」(見開き設定、**既定 OFF**) で行い、PDF と EPUB の両方に効く。本ごとの保存値があればそれが優先 (§4.3) |
| D11 | WebView2 のレジストリポリシー (`AdditionalBrowserArguments`・`UserDataFolder` 等) は**検出せず、設定されていれば従う**。個人用アプリであり、ポリシーを書けるのは管理者か利用者本人のため。`WEBVIEW2_*` 環境変数は偶然の引き継ぎを防ぐため除去を続ける。プライバシーの説明に「ポリシー設定時はそれに従う」と書き添える (§4.5) |
| D12 | (利用者合意 2026-09-26) D2 の例外として、利用者が**明示的に選んだときだけ**元 EPUB と同じフォルダへ同名の PDF を保存する変換を設ける。入口は開くときの確認画面のボタン「PDF ファイルとして保存して開く」と、一覧の右クリック「変換 > PDF ファイルに変換」(複数選択で一括、RAR→ZIP の sibling / batch 変換と同じ構成)。既存ファイルは上書きしない (同名 PDF があればボタンを無効化し理由を示す)、公開前に検証、書込不可ならキャッシュ変換を案内。保存した PDF は通常の PDF (キャッシュ世代を作らず、方向は PDF の `/ViewerPreferences` に書く)。D5 により同名 EPUB は一覧で隠れる。EPUB の「確認せず変換する」設定時の自動変換は従来どおりキャッシュ。S3 で実装、プライバシーの記述は S5 |
| D13 | (利用者決定 2026-09-27) EPUB の開き方は RAR / 7z / LZH の `archive_file_handling` と分離し、専用の `epub_file_handling` (Ask / Convert / Ignore、既定 Ask) で決める。EPUB は未リリースなので旧書庫設定から引き継がない。確認画面の「次回から表示しない」も EPUB 設定だけを Convert にする。Ignore は一覧・フォルダ移動・検索結果の EPUB を除き、変換済みの本も開かない。**公開時は `Cargo.toml` の版番号を旧リリース 4.1.0 より上げることが前提**。`settings_db` は保存した版が実行中の版より新しい場合だけ旧版の読み書きを止めるため、開発中の同一版番号では新設定を旧バイナリから保護できない |

設定変更時の一覧更新は、EPUB と RAR / 7z / LZH で同じ環境設定 OK の再読み込み経路を使う (2026-09-27 方針変更)。通常フォルダはその場で再読み込みし、検索結果・閲覧履歴などは専用の即時再構築をしない。EPUB の「無視する」による履歴除外は、次に履歴一覧を作るときに行う。

## 2. 利用者から見た挙動

1. フォルダに `book.epub` があると、一覧に 1 冊の **PDF 系の本**として出る (バッジ「EPUB」)。未変換ならアイコン、変換済みなら 1 ページ目のサムネイル。
2. EPUB を開くと、未変換なら設定に応じて確認ダイアログを表示するか省略し、変換 (進捗・キャンセル可) して開く。「無視する」では開かない。変換済みの本は「無視する」以外なら即座に開く。確認の要否は EPUB 専用の `epub_file_handling` (Ask / Convert / Ignore、既定 Ask) に従う。どの入口 (クリック・フォルダ移動・履歴・しおり・コレクション・スマートフォルダー・起動復元) から開いても同じ。
3. 開いた後のアドレスバー・親へ戻る・次回起動の復元・履歴・しおりは、すべて `book.epub` を指す。PDF の本と同じ種類として扱われる。
4. DRM 付き / 壊れた EPUB / WebView2 Runtime なし / タイムアウトは、それぞれ理由を明示したエラー。空の本にはしない。
5. 設定「PDF / EPUB の右開き指定に従う」(既定 OFF) を ON にすると、本に保存された見開き設定が無いとき、右開き指定のある
   PDF / EPUB を右開きで開く (EPUB の `page-progression-direction`、PDF の `/ViewerPreferences /Direction /R2L`)。
   OFF (既定) では従来どおりアプリの既定の見開き設定で開く。
6. **一度読み込んだ本は、その実行中は同じ変換結果を表示し続ける。**
   - 「読み込んだ」には、開いたときだけでなく、一覧のサムネイル表示や隣の本の先読みで変換結果を読んだときも含む。
   - mIV の実行中に元の EPUB を差し替えても、既に読み込んだ本は差し替え前の変換結果のまま。差し替えは次回起動以降に
     読み込むときに検出され、開くときに再変換を確認する。まだ読み込んでいない本は、読み込む時点の元ファイルで判定する。
   - キャッシュ管理画面で EPUB の変換結果を消すと「削除予約」になる。その実行中は (まだ表示していない本も含めて)
     変換結果を読める。ファイルと行が消えるのは、同じデータフォルダを使う mIV がすべて終了した後の起動時。

## 3. 中心設計

### 3.1 考え方: 論理パスは EPUB、PDFium が読むのは変換世代のファイル

EPUB を「**中身のバイト列が変換キャッシュに置いてある PDF の本**」として扱う。

- アプリ内で本とページを識別するパス (論理パス) は、**どこでも `x.epub`**。一覧のタイルは `GridItem::PdfFile(x.epub)`、
  中身は `GridItem::PdfPage { pdf_path: x.epub, .. }`、`current_folder = x.epub`。PDF の本と同じ種類。
- PDFium にファイルを開かせる処理は、すべて `pdf_loader` の公開関数 → `encode_*_request` → ワーカープールを通る
  (`src/pdf_loader.rs:884-964,3919-4640`)。論理パス → 変換世代ファイルの解決は `pdf_loader` の入口で行う (§3.3)。
  例外の `render_page_async` (`pdf_loader.rs:4305`、プロセス内の別 PDFium ワーカーへ論理パスを直接送る) は
  `pdf_loader` の外から使われていないので削除する。
- PDFium 以外で本のファイル自体を扱う処理 (タイルの「外部で開く」`external_tool.rs:223`、ドラッグ出し `ui_main.rs:1917`、
  コピー・削除・改名) は、**論理パス = 元 EPUB を扱うのが正しい**。これらは変更しない。内容同定のハッシュ
  (`content_identity.rs:1786`) は、その実行で本を固定済みなら固定世代の値、未固定なら元 EPUB から計算する (§4.1)。PDF ページの外部渡しは PNG 化 (`materializer.rs:244-249`) なので描画は `pdf_loader` を通る。

### 3.2 不変条件

- **I1 識別の一意性**: 本・ページの識別・永続キー・表示・履歴に使うパスは `x.epub`。変換世代ファイルのパスは
  `pdf_loader` の要求の中とキャッシュ管理の外へ出さない (ログを除く)。
- **I2 解決は `pdf_loader` の入口で**: `x.epub` → 変換世代ファイルの解決は `pdf_loader` の解決関数だけが行う。
  以後の要求 (open admission の文書 ID、列挙の合流キー、ワーカーの文書キャッシュ) はすべて**実際に読むファイル**で動く。
- **I3 未変換は型付きの結果**: 有効な変換世代が無い / 元 EPUB の mtime・size が記録と違う場合、解決は
  `PdfReadError::NotConverted` を返す。この型は非同期列挙の配布 (`pdf_loader.rs:4511`、現状は `io::Error` を文字列から
  作り直す) でも失わない。空の本・黙った fallback にしない。
- **I4 派生データは世代スタンプで検証**: PDF の中身から作った派生データの鮮度判定には、EPUB では**再利用されない世代 ID**
  を含むスタンプを使う (§3.4)。
- **I5 UI スレッドで DB もファイルも引かない**: 解決は UI スレッドで行わない。
- **I6 世代は不変で、プロセス実行中は消さない**: 一度公開した世代 (ファイルと世代表の行、§4.4) は書き換えない。廃止した
  世代は削除予約表に記録し、次回起動時の削除ゲート (§4.4) で消す。
- **I7 一度解決した本は実行中固定**: 1 回の実行中、論理パス `x.epub` に対応する世代は**最初に解決したら変えない**
  (元 EPUB がその後書き換わっても、削除予約されても、別インスタンスが新世代を公開しても変えない)。まだ解決していない本は、
  最初に解決する時点の元ファイルの状態と DB の現在表 (I8) で判定する (実行開始時点のスナップショットは取らない)。第 4 版は「元 EPUB の書き換えは開いている PDF の書き換えと同等」としたが、レビュー 4 回目で
  同等でないと判明した (普通の PDF はワーカーが次の要求で開き直すが、EPUB は未描画ページが `NotConverted` になり得た)
  ため、実行中は元 EPUB の変更を見ない規則に改めた。
- **I8 状態遷移は 1 つ** (保存先は §4.4 の `epub_cache.db`):
  - 世代表: 世代 ID ごとに不変の 1 行 (元パス・元の状態・ハッシュ・PDF ファイル名・サイズ・綴じ方向)。
  - 現在表: 元パスごとに「現在の世代 ID」を 1 つ指す。
  - 変換の公開 (世代 G、G の元状態 s): 元 EPUB を**書き込み共有を許さないモード**で開く (`BEGIN IMMEDIATE` の書き込み
    ロックを長く持たないよう、ハンドルはトランザクションの前に取る) → 1 トランザクションの中でそのハンドルの状態を確認し、
    コミットまでハンドルを保持する (確認から DB 更新まで書き換えを排除する)。
    - **どの結果でも、まず G を世代表へ不変行として挿入する** (ファイルは既に公開済みなので、削除ゲートが行からファイル名を
      引けるようにする)。
    1. 元 EPUB の現在の状態 ≠ s → **古い変換**。G を削除予約へ回し、現在表は変えずに `Stale` を返す (変換中に元が変わった。
       呼び出し側は必要なら新しい状態で変換し直す)。
    2. 現在表が指す世代が状態 s で、そのファイルが存在する → **先に公開した側が勝ち**。G を削除予約へ回し、既存を採用する。
    3. それ以外 (現在表なし / 状態が違う / 指す世代のファイルが欠落) → 現在表を G へ向け、旧世代 (あれば) を削除予約へ回す。
    - 公開時に元の状態を確認するので、現在表は常に「公開時点の元 EPUB の状態」の世代を指し、古い変換が新しい有効世代を
      上書きすることはない。
  - 欠落の検出 (解決時に現在表の世代のファイルが無い): 1 トランザクションで、**現在表がまだその世代を指していれば現在表の
    行を消し**、その世代を削除予約へ回す (既に別の世代を指していれば何もしない)。以後の解決は `NotConverted` になり、
    次の変換は規則 3 で正常に公開される。
  - 削除予約 (管理画面の削除・全削除・元ファイル消失の掃除): 1 トランザクションで、**その時点で現在表が指している世代**の ID を
    削除予約表へ入れる。現在表と世代表は変えない。その後に別の mIV が公開した新しい世代は、この削除の対象にならない。
  - 削除ゲート: 削除予約表の各世代について、ファイルと世代表の行を消し、現在表がその世代を指していれば現在表の行も消す。
  - 解決は常に**その時点の現在表**を使う (削除予約の有無は見ない)。一度解決した本は I7 により固定表の世代 ID を使い、
    現在表が変わっても世代表の自分の行 (ゲートまで残る) を引く。
  - SQLite のトランザクションはプロセスをまたいで直列化されるので、データフォルダを共有する複数の mIV の公開と削除予約は
    この規則で調停される。

### 3.3 解決とメモ (I7)

- 解決関数 `pdf_loader::resolve_read_target(logical) -> Result<ReadTarget, PdfReadError>` を、各公開関数
  (`get_document_info` / `get_page_sizes` / `analyze_page_content_type` / `enumerate_pages*` / `render_page*`) の先頭で呼ぶ。
  `ReadTarget { read_path, stamp: DocumentStamp }`。
  - 拡張子 pdf → `read_path = logical`、スタンプは論理パスの (mtime, size) (現状と同じ)。
  - 拡張子 epub → プロセス内の**固定表**を論理パスで引く → あれば元 EPUB を stat せずにそれを返す (I7)。
    無ければ元 EPUB を stat し (時刻は `SystemTime` の完全な精度)、`epub_cache.db` の現在表が指す世代 (I8) の元の状態
    (完全精度の時刻・サイズ) と照合し、**世代ファイルの存在を確かめてから**固定表に**挿入のみ** (既にあれば既存を使う =
    先勝ち、置き換えない)。固定表の値は世代 ID。現在表に行なし・状態不一致 → `NotConverted`。世代ファイルが無い →
    I8 の「欠落の検出」を行ってから `NotConverted` (壊れた世代を読み続けず、次の変換を妨げない)。
- 固定表は置き換えを持たないので、「解決 A が旧行を読み、公開 B が切り替え、A が旧値をメモへ戻す」という逆流
  (レビュー 3 回目) が起きない。変換 (新世代の公開) は、その状態について `NotConverted` を返した後にだけ起こるので、
  同じ状態に 2 つの世代が並ぶこともない。変換完了時は、公開した世代を固定表へ挿入してから開き直す。
- 削除予約は I8 のとおり現在表と世代表を変えず、固定表も変えない。I6 によりファイルは存在するので、その実行中はまだ解決していない
  本も含めて読める。
- `enumerate_pages_async` は、解決・実ファイルのパスでの合流登録 (`pdf_loader.rs:4337,4423`)・要求の組み立てを
  **すべて背景スレッドで一体に**行う。UI 側は背景処理の開始とハンドルの受け取りだけにし、登録前のキャンセルと
  起動失敗でも待ち手を必ず完了させる既存の契約 (`pdf_loader.rs:4688`) を守る。
- 変換器が出力した `.part` の検証は、論理パスの API に混ぜず、変換処理専用の物理パス検証関数
  (`pdf_loader::verify_converted_pdf(path, expected_pages)`) で行う。

### 3.4 文書スタンプ (I4)

- **EPUB のスタンプ**: `(世代 ID, 世代ファイルのサイズ)`。世代 ID は `epub_cache.db` の世代表 (`AUTOINCREMENT`、§4.4) で
  予約する 64bit の値で、行を消しても再利用しない (時刻由来にしない。同時変換や時計の逆行で重複するため)。既存の DB 列 (`catalog.rs:692` 等の `mtime, file_size` の整数ペア) にそのまま
  格納する (EPUB の行では mtime 列の意味が「世代 ID」になる。行は論理パスごとに分かれているので PDF の値と混ざらない)。
  秒へ切り捨てた mtime (`pdf_loader.rs:431,550`) に依存しないので、同一秒・同一サイズの衝突が起きない。
- **PDF のスタンプ**: 従来どおり (挙動不変)。
- **契約**: 1 つの処理 (サムネイル 1 枚の読み込み、`pdf_meta` の補完、先読み、詳細表示のページ数等) は、処理の最初に
  `resolve_read_target` を 1 回呼び、その `ReadTarget` のスタンプでキャッシュを照合し、同じ `ReadTarget` で描画・列挙し、
  結果にも同じスタンプを付けて保存する。I7 により一度解決した本は同一実行中に同じ世代が返るので、処理の途中で世代が
  入れ替わることはない。
- **表示用のファイル属性とは分ける**: ページの `image_metas` (詳細列・ツールチップの更新日時とサイズ、`app.rs:26771-26790`、
  `ui_main.rs:16340-16365,17649-17678`) は**元 EPUB の属性**のまま (表示用)。世代スタンプは `image_metas` に入れず、
  キャッシュ照合の経路だけで使う。EPUB の要求は「スタンプはワーカーで解決」の印を持ち、`image_metas` の値を照合に使わない。
  `image_metas` から要求を作って照合する入口 (auto-aspect の初期シード、編集プレビュー) は、EPUB について解決済みスタンプを
  使うか、先行ヒットを行わない。実装 PR で照合入口を全数列挙する。
- UI スレッドで要求を組み立てる箇所 (タイルのサムネイル要求は一覧で採った元ファイルの値を引き継ぐ、`app.rs:79419`) は、
  EPUB について「スタンプはワーカーで解決」と印を付けて渡し、ワーカーが `resolve_read_target` の値で照合する。

対象 (レビュー 1〜3 回目で判明したもの。実装 PR で全数を表にする):

| 派生データ / 処理 | 現状の鮮度判定 | 位置 |
| --- | --- | --- |
| 親フォルダのページ数キャッシュ (`pdf_meta`) の読み書き | 読み出し = 論理 stat、書き込み = 読んだファイルの stat で不一致 | `app.rs:26695-26743,26908` |
| `pdf_meta` の catch-up・隣接先読み | 論理 EPUB を stat してから PDF API | `thumb_loader.rs:1974-1982,2070` |
| 一覧タイル・ページのサムネイル要求 | 描画前に要求の mtime/size で照合 | `app.rs:79419`、`thumb_loader.rs:329,1185,2755` |
| サムネイルカタログ (本ごとの DB、`page_NNNN` 行) | 行の mtime/size | `catalog.rs:21-33,692` |
| 代表サムネのピンの source ID | `PdfPage` の元パスを stat | `folder_thumb_pins.rs:586,596` |
| 編集プレビューのコンテナ検証 | コンテナの stat、描画前に要求値で照合 | `thumb_loader.rs:1107-1120` |
| auto-aspect の初期シード | `image_metas` から要求を作りキャッシュの mtime・size と直接比較 | `app.rs:18171,18199` |
| 詳細表示のページ数 (DB 読み書き) | 元ファイルの stat | `app/metadata_ops.rs:1417,1456` |
| 外部ツール用ページ PNG | 元ファイルの時刻で再利用 | `materializer.rs:554,571` |
| リモートのページ数・`PdfIdentity` (メモリ・DB) | 論理パスの mtime/size | `remote_ipc/container.rs:6126,6144` |
| 類似画像索引の早期再利用 | 元ファイルの mtime/size | `similar_index.rs:6052` (EPUB 投入可否 `:5752`) |
| 保持ラスタ / final AI | ページ識別のみ | `app.rs:65221-65261` (プロセス内のメモリだけに持ち、I7 により一度解決した本の世代は実行中変わらないので追加不要) |
| PDF ワーカーの文書キャッシュ | `(path, password, mtime, size)` | 実ファイルのパスで動くので自動的に満たす |

### 3.5 この方式で満たされること

- D3 のキーは `x.epub::page_N` (`edit_source::page_key_for_pdf`、`edit_source.rs:394-396`)。本単位のキー
  (`current_folder` 由来の見開き・読書位置・代表サムネ、`app.rs:21278-21294,44021,21351`) も `x.epub`。
- 本の種類はタイルも中身も PDF。`GridItem` で分岐する箇所 (レーティング種別 `app.rs:56296`、履歴 `:44181,78810`、
  しおり `BookContainerKind::Pdf` `:35346`、代表サムネの許可規則 `:79613`、内容同定 `content_identity.rs:139-154`) は
  PDF の本と揃う。拡張子で再分類する箇所は §4.1 の一覧で `is_paged_document_path` へ寄せる。
- mIV 内での移動・改名は既存の汎用キー移行が追従する (`.pdf` 限定はパスワード移行のみ、`rename_key_migration.rs:1314,1334,2022`)。
  キャッシュ行は元パスのハッシュで決まるので、改名後は再変換になり、旧行は「元ファイル消失」の掃除で削除予約になる。
- `archive_source_override` を使わない。EPUB を開くとき `start_loading_items` が残存 override を消すこと
  (`app.rs:27737-27742,27923-27933`) をテストで固定する。
- 別ウィンドウの記述子 `ViewerContextDescriptor::Pdf` (`app.rs:2633`) は変更不要。detached 凍結対象の経路に分岐を足さない。

### 3.6 却下した案

| 案 | 内容 | 却下理由 |
| --- | --- | --- |
| A | `PdfPage` に識別用パスの欄を足す | 意味的変更 60〜90 か所。保存済みキーから組み立て直す経路が描けない項目を作る危険が高い |
| B | `archive_source_override` を PDF にも使う | ページのキーが変換 PDF のパスのまま (D3 を満たさない) |
| C | 変換 7z と同じく、キーは変換 PDF のパス | 元 EPUB の移動・改名でキーが孤立。サイドカーが APPDATA に書かれる。ドライブ文字違いが同じキャッシュを共有 |
| 第 1 版 | タイル `ConvertibleArchive` + 中身 `PdfPage`、`encode_*_request` 内の可変パス表で解決 | 本の種類の分裂、UI スレッドでの解決、固定名キャッシュの上書きと読者の競合 (§8 R1, R2) |
| 第 2 版 | ディスパッチャで解決 + 読み取りリース | open admission の判定順序と矛盾、リース登録と廃止が非原子的、PDFium ワーカーが旧文書を保持 (§8 S1, S2, S7) |
| 第 3 版 | 実行中に同じ EPUB の世代を切り替え、メモを無効化で追従 | 開いている本での世代混在、メモへの旧値の逆流、1 処理内での世代の食い違い (§8 T1, T2, T5) |

## 4. 構成要素

### 4.1 一覧・分類

- 一覧の分類 (`src/app/folder_scan.rs:547-602`) で `.epub` を `GridItem::PdfFile(path)` にする。
- 変換処理の入力種別は**別の列挙** `ConvertSource::{Archive(ArchiveFormat), Epub}`。`ArchiveFormat` には足さない
  (網羅 match `archive_converter.rs:316,782,977` に「呼ばれてはいけない枝」を作らないため)。`from_extension` /
  `CONVERTIBLE` も変えないので、`ConvertibleArchive` にもリモートのアーカイブ変換ジョブ (`remote_ipc/archive_job.rs:1702`) にも
  入らない。EPUB の変換結果は `archive_cache` ではなく `epub_cache` に置く (§4.4) ので、`archive_cache` の `format` 列は変えない。
- タイルのサムネイル: 既存の `PdfFile` と同じ要求 (`pdf_page: Some(0)`、`app.rs:79426-79433`)。未変換なら `NotConverted` で
  アイコン + 「EPUB」バッジ。**`NotConverted` を失敗としてログに積まない** (未変換は正常状態)。
- 同名スキップ (D5): `skip_epub_if_pdf_exists`。stem が同じ `.epub` と `.pdf` の `PdfFile` で EPUB を隠す (§4.6)。
- 拡張子で「PDF の本か」を判定している箇所は新設 `folder_tree::is_paged_document_path` (pdf / epub) へ寄せ、
  「実ファイルが PDF か」の問い (パスワード関連) は `is_pdf_extension` のまま残す。判明している再分類箇所:
  - コレクション: `collection_store/prepare.rs:680,692` (現状 `.epub` は `Unsupported`)
  - タグ一覧: `tag_view.rs:472,488` (現状 `Folder` 側へ落ちる)
  - Remote Web: `crates/remote-web/src/store.rs:306,312,676`
  - 詳細欄の種別表示: `ui_main.rs:4610,4650` (現状 `EPUB PDF` と表示)
  - PDF パスワード: `app.rs:26379` (別 PDF のセッション値へのフォールバック) と `:26982` (エラー判定)。EPUB には適用しない
  - 内容同定: `ContentKind::Epub` を足す (未リリースなので移行不要)。対象は `content_identity::from_path`
    (`content_identity.rs:100-118`) と、`GridItem::PdfFile` / `PdfPage` から種別を決める箇所 (`content_identity.rs:139-154`、
    現状は常に `Pdf`)。読書位置の更新時の `record_current_container_content_identity` (`app.rs:44027-44032,62003-62044`) も
    これで EPUB を扱う。EPUB の同定に必要な値 (サイズ・時刻・全体 SHA-256・**先頭 64 KiB + サイズのハッシュ**、
    `content_identity.rs:360,1223,1247`) の取り方:
    - その実行で本を**固定済み** → 現在のファイルを読み直さず、固定表の世代 ID で世代表の行から取る (複写時に全部計算して
      保存済み。表示中の内容と同定値が一致する)。
    - **未固定** (未変換・まだ読み込んでいない。例: 物理フォルダ一覧から起動する既存編集の台帳 backfill、
      `app/content_identity_detection.rs:227`) → 現行の PDF と同じく元 EPUB を読んで計算する (`content_identity.rs:1768-1786`)。
    - 世代表の時刻は FILETIME ticks (§4.4)。既存の内容同定台帳は Unix ナノ秒 (`content_identity.rs:1160,1871`) なので、
      台帳へ渡すときに明示的に変換する (範囲外は台帳側の既存の扱いに従う。変更検出には使わない)
    - **非同期の backfill と固定の競合**: backfill の要求に、分岐した時点の来歴 (「固定世代 ID」または「未固定 + 元の状態」) を
      持たせる。台帳へ書く直前に、その本の現在の固定状態・元の状態と照合し、食い違えば書かない (遅れて完成した元ファイル側の
      観測が、固定世代の編集記録を同じ `file_key` で上書きしないため。`content_identity.rs:1703,1829`、
      `app/content_identity_detection.rs:197`)
  - その他、非テストの拡張子判定 約 47 か所は実装 PR の分類表で振り分ける (件数ではなく項目で突き合わせる)

### 4.2 開く流れ (型付きの結果と owner)

- PDF の本を開く処理の結果を型で表す: `PdfOpenFailure::{PasswordRequired, NotConverted, Other(String)}`。
  非同期列挙の配布 (`pdf_loader.rs:4511`) もこの型で運ぶ (I3)。
- **PDF を列挙する入口は 3 つ**あり、すべて同じ App メソッド `route_pdf_open_failure(owner, logical, failure)` へ送る:
  1. 通常の `load_pdf_as_folder` → `poll_pdf_enumerate` (現状、その他の失敗は「空の本」`app.rs:27026-27035`)
  2. コレクション移動の事前列挙 (`app/collection_navigation.rs:678-683`、現状パスワード以外の失敗は `None` で中止)
  3. スマートフォルダーの事前列挙 (`app/smart_folder.rs:1014-1024`)
- PDF の pending (`app.rs:14423`、現状 path・password・handle のみ) に **open-request owner と履歴 snapshot** を持たせ、
  `NotConverted` のときはその owner のまま EPUB 変換要求へ移す。履歴の巻き戻しとフルスクリーン予約の引き継ぎは、
  既存の `FolderOpenOutcome::ConversionDialogOpened` を受けた呼び出し側の処理 (`app.rs:77968`) と同じ意味で行う
  (列挙完了時に発生する点だけが違う。実装時に同じ遷移を型で表す)。
- 変換要求は `epub_file_handling` に従う: Ignore → EPUB を無視するトースト、Ask → 確認ダイアログ
  (要約は変換器の `inspect` をワーカーで実行: レイアウト・綴じ方向・spine 数・DRM 判定)、Convert → 確認なしで変換。
- 変換ダイアログは既存 `ArchiveConvertState` の相 (`archive_convert.rs:113-188`) を流用し、完了処理は EPUB 専用の分岐
  (変換 ZIP 用の「開いた直後の current_folder 同期確認」`:358-393,1197-1241` と `archive_source_override` は通らない)。
  完了後は同じ owner で元の入口の開き直しを行う。

### 4.3 綴じ方向 (設定で ON、既定 OFF)

- **背景**: PDF には綴じ方向の属性 `/ViewerPreferences /Direction /R2L` (PDF 1.3 以降の仕様) がある。mIV は現状これを
  読んでいない (`src/` に参照なし、2026-09-25 確認)。EPUB の `page-progression-direction` は、変換器がこの PDF 属性へ写す
  (`crates/epub-pdf-worker/src/render.rs:180`)。
- **設定 (D10)**: 見開き設定に「PDF / EPUB の右開き指定に従う」(`follow_document_reading_direction`、既定 false) を追加する。
  右開きでは左右キーの進行方向や見開きの並びが逆になり、利用者によっては自動の切り替えが困る。既定 OFF で既存の PDF の
  開き方を変えない。PDF と EPUB で別の設定にはしない (変換後は同じ PDF の本として扱うため)。
- **方向の取得**: 非同期列挙の結果に方向を含めて UI へ渡す (UI スレッドで別途 `get_document_info` を同期に呼ばない、I5)。
  - EPUB: 変換時に世代表の `direction` 列へ保存した値を使う (解決した `ReadTarget` が保持)。PDF を読み直さない。
  - 普通の PDF: 列挙要求に `want_direction` を付けたときだけ、PDF ワーカーが公開 PDFium 関数で文書をもう一度開いて
    `/ViewerPreferences /Direction` を読む。付けない既定では追加の処理をしない。S2c は D10 設定が ON かつ保存済みの
    見開き設定が無い本に限って付ける。
  - `pdfium-render` の複製 (開いている文書のハンドルから読むため、22MB・667 ファイル) は採らない (S2b の判断)。
- **適用条件**: 設定が ON、かつ本に保存済みの見開き設定が無いときだけ。見開き設定の復元 (`apply_spread_for_key_with_fallback`、
  `app.rs:21375-21407`) は方向の正本が `spread_mode` (`SpreadMode::reading_direction`、`:21403-21405`) なので、既定の見開き
  モードに副作用の無い `SpreadMode::with_reading_direction` (`settings.rs:3039`) を適用する
  (`sync_spread_mode_from_reading_direction` はお気に入りの既定設定も変えるので使わない、`ui_fullscreen.rs:36474`)。
  保存済みの本は保存値が勝つ。
- **仮ページの先出し**: `pdf_meta` による仮ページの先出し (`app.rs:26483,26668`、スマートフォルダーの warm 経路
  `smart_folder.rs:1014`) は列挙より前に見開きを復元し、ページ数が同じなら列挙後に再設置しない (`app.rs:26943,28130`) ため、
  方向が間に合わない。**設定が ON かつ保存済みの見開き設定が無い本に限り**、先出しを使わず列挙応答を待って設置する
  (PDF・EPUB とも)。設定 OFF の既定では、PDF の先出しは従来どおり。EPUB の先出しの扱いは S2 で表示待ち時間を計測して決める
  (PDF 文書の open は中央値 654 ms・p90 6,393 ms の実測がある、`backlog-on-hold.md:38`)。
- 設定の追加箇所は既存の見開き設定と同じ並び (`settings.rs`、環境設定の見開きページ、検索索引、マニュアル `settings.html`)。

### 4.4 EPUB 変換キャッシュ (`epub_cache`) と削除ゲート

RAR/7z の `archive_cache` には相乗りせず、**専用の保存先と DB を新設する** (第 6 版までは相乗り案。レビュー 6 回目の指摘で
変更、§8 W1〜W6)。理由:
- リリース済みの旧版 mIV (旧ポータブル版など) が同じデータフォルダを使うと、旧版の `clear_all` (`archive_cache.rs:424-429`) は
  形式を問わず全行のファイルを即時削除し、I6 を破る。旧版が知らない場所に置けば触られない。
- 未リリースなので、世代表・現在表・削除予約表を最初から正しい形で作れる (既存行への `ALTER` が要らない)。
- 容量上限 (D4) の対象外であることが構造で決まる (`archive_cache` の合計・候補・`total_size()` に EPUB が入らない。
  `archive_cache.rs:455-538`、`cache_maintenance.rs:192`)。

構成:
- 保存先: `<data_dir>/epub_cache/<hash[..2]>/<hash>/<stem>.g<世代 ID>.pdf` (hash は元パスの正規化の SHA-256、既存と同じ規則)。
- **世代 ID は変換を始める前に予約する**: 発番表 `generation_ids (generation_id INTEGER PRIMARY KEY AUTOINCREMENT, reserved_at)`
  に 1 行挿入して ID を得て、その ID で `.part` とファイル名を決める。公開されなかった ID (取消・失敗・`Stale`・負け) は
  欠番になるだけで再利用しない。
- DB: `<data_dir>/epub_cache.db` (**PDF を置く `epub_cache/` の外**。`archive_cache.db` と同じ置き方、`archive_cache.rs:28`。利用者がフォルダを丸ごと消しても発番が 1 から戻らず、`<data_dir>/cache` に残るサムネイル・`pdf_meta` と世代スタンプが衝突しない)。
  - `generations` (世代表): `generation_id INTEGER PRIMARY KEY` (発番表で予約済みの値)、`src_path_key`、`src_path`、
    `src_size`、`src_mtime_ticks` (Windows FILETIME の 100ns 単位。全範囲を失わずに表せるので、元ファイルの変更検出 (I3) の
    比較はこの値で行う)、`src_sha256`、`src_head_hash`
    (先頭 64 KiB + サイズ、内容同定の既存規則)、`pdf_file`、`pdf_size`、`page_count`、`direction`、`profile`
    (例 `reflow-v1`)、`created_at`。**行は不変**。
  - `current` (現在表): `src_path_key PRIMARY KEY` → `generation_id`、`last_access_at`。
  - `retired` (削除予約表): `generation_id PRIMARY KEY`、`retired_at`。
- 状態遷移は I8 のとおり。**公開**: 検証済みの `.part` を `replace_file_atomic` (`archive_converter.rs:726-742`) で
  **上書きなし (`no_clobber`)** で世代ファイルへ移す → `BEGIN IMMEDIATE` で I8 の公開規則 (元状態の再確認、先勝ち、
  古い変換の破棄) → 採用した世代を固定表へ挿入。
- **変換入力の固定**: 変換の前に元 EPUB を作業フォルダへ複写する。複写は元ファイルを**書き込み共有を許さないモード**
  (`FILE_SHARE_READ` のみ) で 1 つのハンドルから読み、その間の書き込みを排除する (書き込み中で開けなければ
  「ファイルが使用中」の型付きエラー)。同じハンドルで複写前の状態 (完全精度の時刻・サイズ) を取り、複写しながら
  全体 SHA-256 と先頭 64 KiB + サイズのハッシュを計算して世代表へ保存する (内容同定に使う、§4.1)。変換器は複写だけを読む
  (`package.rs:529,534`)。複写は取消・失敗時も作業フォルダごと回収する。
- **削除ゲート (起動時)**: 単一インスタンスの取得後、コレクション DB・Remote IPC の受付・`App` の生成より前
  (`lib.rs:1293` の手前) に置く。
  1. データフォルダ単位の生存ロック `<data_dir>/epub_cache/.alive` を**排他**で非ブロッキング取得を試みる。取れた = 同じ
     データフォルダを使う新版の mIV が他に動いていない (ポータブル版とインストール版の併用、`--data-dir` 指定の別インスタンス
     を含む。`single_instance.rs:65-70`、`data_dir.rs:24`)。
  2. 取れたらスキーマを作成・更新し、削除予約を実行する (パスが `epub_cache` 配下であることを検証してから消す)。併せて
     **予約で追跡しているファイルだけ**を回収する: 世代 ID の予約時に予定パスを記録し、公開に至らなかった予約 (変換中のクラッシュ・強制終了) の世代ファイル・`.part`・`.part.tmp-*` を消して予約を閉じる。閉じた予約行も消す (ID の再利用は SQLite の AUTOINCREMENT が防ぐ)。予約の無いファイルを探すための全体走査はしない (起動時間を冊数に比例させないため。キャッシュにファイルを作るのはこの変換経路だけなので、予約の無いファイルは外部のプロセスか未出荷の開発版の残りに限られる。S2a の独立レビュー 2 回目の指摘を受けた設計判断)。削除は、ルートから対象までの各フォルダと対象をハンドルで開いて再解析ポイントでないことを確かめ、同じハンドルで行う (確認と削除の間に利用者のデータフォルダへ書き込むプロセスは脅威モデルの対象外)。取れなければ
     削除は延期する (スキーマは先に起動したインスタンスが作成済み。未作成なら EPUB を無効にする)。
  3. その後、生存ロックを**共有**で取り直し (他のインスタンスの削除ゲートが走っている間は待つ)、プロセス終了まで保持する。
     共有の取得に失敗したら、その実行では EPUB の解決・変換・キャッシュ操作を無効にし (`NotConverted` ではなく専用の
     エラーで理由を示す)、成功扱いにしない。共有ロックが成立するまで、変換とキャッシュ操作も開始しない。
  - 他のインスタンスが動いている間、削除予約は**同じデータフォルダを使う mIV がすべて終了した後の起動**まで延期される。
  - Remote のサービスは別プロセスだが PDF をコアへの IPC で描く (`remote_ipc/service.rs:434`、`container.rs:6171`) ので、
    生存ロックはコアだけが持てばよい。
  - **旧版との同時使用**: 旧版は `epub_cache` を読み書きしないので I6 を破らない。旧版で EPUB を開いた場合は従来どおり
    非対応ファイルとして扱われるだけ。
- キャッシュ管理画面 (`archive_cache_manager.rs`) に EPUB の区画を足す (`epub_cache.db` の現在表と世代表を一覧し、状態に
  「削除予約」を出す)。RAR/7z の容量表示・容量上限には含めない。

### 4.5 変換器ワーカー (`crates/epub-pdf-worker`)

本体からは子プロセスとして 1 冊ずつ起動する (プールにしない)。スパイクからの変更点:

- **ネットワーク遮断**: `WebResourceRequested` で仮想ホスト以外への要求をすべて拒否する。
- **本のスクリプトを実行しない**: `IsScriptEnabled = false`。待機判定の `ExecuteScript` がこの状態で動くかを S1 で確認する。
- **待機**: `PeekMessage` + 10ms sleep を `MsgWaitForMultipleObjects` のタイムアウト付き待ちへ替える。
- **進捗**: 標準出力に 1 行 1 JSON。本体は既存の進捗バーへ流す。
- **出力**: 本体が指定した `<世代ファイル>.part` へ書く。本体が `verify_converted_pdf` で検証してから §4.4 の手順で公開する。
- **ユーザーデータフォルダ**: `<temp_root>/epub-<pid>` (materializer の一時領域、`materializer.rs:340-353`)。死んだ PID の掃除 (`:1496-1586`) に含める。
- **子プロセスの後始末 (S1 の合格条件)**: 本体は変換器を `CREATE_SUSPENDED` で起動 → `KILL_ON_JOB_CLOSE` の Job Object へ
  割り当て → 再開する。WebView2 の各 PID が同じ Job に属すること、キャンセル (Job 終了) と本体の強制終了で全プロセスが
  消えることを実測する受入試験を S1 の完了条件にする。成り立たなければ設計を差し戻す (黙った代替策を入れない)。
- **リフローの組版**: 電子書籍端末相当の固定値 (候補 720×1024 CSS px、余白 32px。草枕・moby-dick・ごん狐を画像化して決める)。
  変換プロファイル名 (例 `reflow-v1`) をレポートに出す。
- **終了コード → 本体のエラー**: 2 DRM / 3 不正 / 4 WebView2 Runtime なし / 5 描画失敗 / 6 タイムアウト / 8 WebView2 Runtime に必要な API (`ICoreWebView2_22` 等) が無い、を型付きエラーへ。7 は D11 で廃止した (欠番。受け取ったら想定外の終了コードとして扱う)。
- **起動時の環境**: 本体は変換器を起動するとき `WEBVIEW2_*` の環境変数 (追加引数・ユーザーデータ先・ブラウザ実行ファイル先 等) を外して渡す (変換器側でも外す)。レジストリのポリシーは D11 により検出せず従う。ポリシーで一時データの保存先が変わった場合は、変換器がレポートに記録して続行し、自分で作ったフォルダ以外は消さない。

### 4.6 同名スキップ設定 (D5)

`skip_epub_if_pdf_exists` (既定 true)。`skip_archive_if_zip_exists` と同じ箇所に並べる:
`settings.rs:4515` 付近と `Default`、`folder_scan.rs:258-264,790-853`、`folder_tree.rs:25-48,701-738`、
`smart_folder.rs:3503-3769`、`settings_db.rs` のリモート一覧設定 5 か所 (`:54-61,298,354,375,2923`)、
環境設定 `preferences/pages.rs:8603-8609`・検索索引 `search_index.rs:534-539`・再読込タプル `preferences.rs:2291-2299,2591-2599`。

### 4.7 リモート (mIV Remote)

- 初版: **変換済みの EPUB を閲覧できる**ところまで。
- コア側: タイルは `PdfFile` なので `Pdf` として送られる (`remote_ipc/container.rs:3455`)。PDF 判定 (論理パスの拡張子、
  `container.rs:3139,3300,4384,4460,5643,5671,7041-7069` 付近) を `is_paged_document_path` へ置き換え、アーカイブ分岐が
  先に来る箇所 (`:4458,5633`) で EPUB がアーカイブ扱いにならないことを確認する。読み取りは `pdf_loader` の解決を通る。
  ページ数・`PdfIdentity` のキャッシュ (`:6126,6144`) は §3.4 のスタンプを使う。
- Remote Web 側: `crates/remote-web/src/store.rs:306,312,676` の拡張子による再分類に EPUB を足す。`web/app.js` の種別一覧も同様。
- EPUB はアーカイブ変換ジョブの対象外 (§4.1)。未変換の EPUB を開くと「PC で一度開いて変換してください」。

### 4.8 配布

mIV Remote のサービス exe と同じ形 (cargo でビルド、launcher が内包、ポータブルは exe の隣、本体は自分の exe の隣から探す。
`src/remote_ipc/service.rs:41-46` と同型 + `MIV_EPUB_PDF_WORKER` での上書き)。

触る箇所: `crates/launcher/build.rs:31-64,112-117`、`crates/launcher/src/main.rs:23,87-91` (+ テスト `:584-610`)、
`scripts/build-release.ps1` / `.sh`、`scripts/build-dev.ps1`、`scripts/build-portable.ps1`、`scripts/build-dist.ps1`
(ビルド・存在確認・複製・署名一覧・プロセス停止一覧・clean・最終 PE 一覧)、`installer/readme*.txt`、
`installer/mimageviewer.iss` のコメント、`CLAUDE.md` Distribution 節、`docs/portable-build-plan.md`。
x64 で `+crt-static` なので VC runtime 検査の追加設定は不要 (`check-vcrt-pe-dependencies.ps1:67-73`)。

### 4.9 文書

- マニュアル: `formats.html:167-179` と `faq.html:90,168-176` の「EPUB 非対応」を書き換え (DRM のない EPUB を固定レイアウトへ
  変換して閲覧。文字サイズ変更・テキスト選択は不可。朗読音声・動画は含まれない。キャッシュの削除は次回起動時)。
  `settings.html` (同名スキップ)。
- 製品ページ `index.html:54,1129`。対外表記は「DRM のない EPUB を変換して閲覧」。
- `privacy.html` / 製品ページ「安心して使えます」: 変換時に WebView2 の一時データを端末内に作ること、通信しないこと。
- 設計文書: `docs/virtual-folders.md` (論理パスと変換世代、世代スタンプ、実行中の固定)、`docs/spec.md`、
  `docs/architecture-overview.md` (新ワーカー、削除ゲート)、`docs/async-architecture.md` (子プロセス + Job Object、世代ファイルの寿命)。

## 5. 実装の段階

| 段階 | 内容 | 完了条件 |
| --- | --- | --- |
| S1 | 変換器の仕上げ (§4.5) と Job Object 受入試験 | 27 冊バッチが前回と同等。外部 URL を参照する合成 EPUB で通信ゼロ。WebView2 全 PID の Job 所属と、キャンセル・強制終了後の消滅を実測 |
| S2 | 解決・固定表 (§3.3)、世代スタンプ (§3.4)、キャッシュ・削除ゲート (§4.4)、型付きの開く結果と owner (§4.2)、綴じ方向 (§4.3) | 状態遷移テスト (§6)、I1〜I8 のテスト、仮ページ廃止による表示待ち時間の perf 計測 |
| S3 | 一覧・分類 (§4.1、分類表付き)、同名スキップ (§4.6)、同じフォルダへの PDF 保存 (D12) | 分類表の全行にテストか理由 |
| S4 | リモート閲覧 (§4.7) | 変換済み EPUB のページ送り・キーが PC と同一 |
| S5 | 配布 (§4.8)、文書 (§4.9) | build-dist が通り、署名・VC runtime 検査に新 exe が含まれる |

## 6. テスト

- 単体: 解決 (pdf / epub 現在世代 / 現在表なし / mtime 不一致 / 世代ファイル欠落)、固定表が先勝ちで置き換わらない、
  削除予約後も同じ実行中は旧世代を返す (まだ解決していない本も)、世代 ID の非再利用、公開トランザクション (現在表の
  置き換えと旧世代の削除予約が同時)、**2 つの DB 接続 (別インスタンス相当) での同時公開で先に公開した世代が採用され、
  負けた側の世代は削除予約へ回る**、**削除予約の後に別接続が公開した世代は削除されない**、固定中の旧世代の内容同定値を
  現在表が変わった後も世代表から引ける、削除ゲート (生存ロックの排他取得・共有保持・取れないときの延期・`epub_cache` 外の
  パスを消さない・未参照の世代ファイルと `.part` の回収)、`archive_cache` の容量表示・上限に EPUB が入らない、
  終了コード → エラー型、同名スキップ、`NotConverted` がログに積まれない、PDF パスワードのフォールバックが EPUB に効かない。
- 状態遷移: 未変換 → 変換 → 開く → キャッシュ削除 (削除予約。同じ実行中は開ける) → 再起動 (削除ゲートで消える) →
  開く (`NotConverted` → 変換) → 開く。保存された DB の行を読み、キーが `x.epub::page_N`、本単位のキーが `x.epub`、
  世代ファイルのパスがキー・表示・履歴に現れないこと (I1) を確かめる。
- 入口: クリック・フォルダ移動 (Ctrl+↑↓)・事前走査付き移動・履歴・しおり・コレクション移動・スマートフォルダー・起動復元の
  それぞれで、未変換 EPUB が「空の本」にならず変換確認に進み、owner・履歴・フルスクリーン予約が保たれること。
- I4: 再起動をはさんでページ数の違う世代へ再変換したとき、§3.4 の表の各派生データが古いまま残らないこと。
- 綴じ方向: 設定 ON で、保存済み見開き設定の無い右開き EPUB / 右開き指定の PDF が右開きモードで開き、保存済みの本は保存値が
  勝ち、お気に入りの既定設定が変わらないこと。`pdf_meta` がある 2 回目の表示でも右開きになること。**設定 OFF (既定) では
  右開き指定の PDF も従来どおりの見開きで開き、先出しも従来どおり動くこと** (既存の挙動を壊さない)。
- 実機 (利用者): 漫画 EPUB を開く→右開き→補正・レーティング→キャッシュ削除→再起動→開き直して編集が残る、DRM 付き相当での
  エラー表示、変換中キャンセルで `msedgewebview2.exe` が残らないこと。

## 7. 実装へ持ち越すテスト (レビュー 8 回目の指示)

1. `epub_cache/` を丸ごと消しても世代 ID が戻らず、サムネイル・`pdf_meta` が新世代に誤一致しないこと。
2. 1601〜1677 年・2262 年以降の時刻を持つ元ファイルでも、変更検出が飽和せずに働くこと。
3. backfill の要求が未固定で分岐した後に本が固定され、元 EPUB が差し替わっても、固定世代の編集記録が上書きされないこと。
4. `Stale` と先勝ちの負けで、世代表の行・削除予約・ファイル回収が揃うこと。

## 8. レビュー記録

### 第 1 回 (2026-09-25、GPT-6 Sol / xhigh、読み取り専用)

判定: 核 (識別は EPUB、PDF 実体は読み取り境界で解決) は妥当。第 1 版のままの実装には反対。

| # | 指摘 | 対応 |
| --- | --- | --- |
| R1 [P1] | タイル `ConvertibleArchive` と中身 `PdfPage` で本の種類が分裂 | タイルも `PdfFile(x.epub)`。拡張子で再分類する箇所を §4.1 に列挙 |
| R2 [P1] | 固定名キャッシュの上書き・削除と PDF 読者の所有関係が未設計 | 最終的に I6 (実行中は消さない) と I7 (実行中は固定) で解消 (第 4 版) |
| R3 [P1] | I4 の対象漏れ | 世代スタンプと 1 処理 1 解決の契約 (§3.4) |
| R4 [P1] | 右開きを表示へ渡す経路が無い | 列挙応答で方向を渡し、既定の見開きモードへ写す (§4.3) |
| R5 [P2] | リモートのタイル種別・分岐順・アーカイブジョブ除外 | `PdfFile` 化と別列挙で除外。Remote Web の再分類 (§4.7) |
| R6 [P2] | 容量上限から除くなら合計からも除く | 合計と候補の両方から除く (§4.4) |
| R7 [P2] | Job Object の前提が未検証 | S1 の受入試験を完了条件に (§4.5) |
| R8 [P3] | 「PDF バイト列はすべて encode 経由」は誤り | 記述を修正 (§3.1) |

### 第 2 回 (2026-09-25、同じ担当・同じ会話の継続)

判定: もう一度設計を詰める必要がある。核は維持できる。

| # | 指摘 | 対応 |
| --- | --- | --- |
| S1 [P1] | ディスパッチャでの解決は open admission の順序と矛盾 | `pdf_loader` 入口で解決 (§3.3) |
| S2 [P1] | DB 行の切替とリース登録が非原子的 | リース廃止。I6 と I7 (第 4 版) |
| S3 [P1] | `defaults.reading_direction` は `spread_mode` に上書きされる | 既定の見開きモードへ写す (§4.3) |
| S4 [P1] | コレクション・タグ・Remote Web が拡張子で再分類 | §4.1 に列挙 |
| S5 [P2] | スタンプの伝達先と永続表現が未定義 | 世代スタンプを既存の整数ペアへ (§3.4) |
| S6 [P2] | 変換導線を通らない入口 | 型付きの開く結果を 3 入口から 1 メソッドへ (§4.2) |
| S7 [P2] | PDFium ワーカーが旧文書を保持、削除の完了状態が曖昧 | 削除予約 + 起動時の削除ゲート (§4.4) |
| S8 [P3] | 詳細欄の表示、PDF パスワードのフォールバック | §4.1 |
| Q6 | `ArchiveFormat::Epub` か別列挙か | 別列挙 `ConvertSource` |

### 第 3 回 (2026-09-25、同じ担当・同じ会話の継続)

判定: もう一度設計を詰める必要がある。核は維持できる。

| # | 指摘 | 対応 (第 4 版) |
| --- | --- | --- |
| T1 [P1] | 開いている本で世代が混在する (ページ一覧は論理パスだけ、描画は毎回解決) | I7: 実行中は同じ EPUB 状態の世代を固定。削除予約も実行中は旧世代を使い続ける (§3.2, §3.3) |
| T2 [P1] | メモの無効化後に旧値が逆流する | 固定表は挿入のみ・先勝ち。変換は `NotConverted` の後にだけ起こる (§3.3) |
| T3 [P1] | 削除位置とプロセス境界 (Remote・App より先の起動、ポータブルと `--data-dir` の併用) | 起動時の削除ゲートを `lib.rs:1293` の手前に置き、データフォルダ単位の生存ロックで延期判定 (§4.4) |
| T4 [P1] | コレクション・スマートフォルダーの事前列挙が変換導線を迂回。型付きエラーが文字列化で失われる。PDF pending に owner が無い | 型付きの開く結果、3 入口から 1 メソッド、pending に owner と履歴 snapshot (§4.2) |
| T5 [P1] | スタンプと描画を別々に解決すると誤ったスタンプを書く | 1 処理 1 解決の契約 + I7 (§3.4) |
| T6 [P2] | 右開きが仮ページの先出しに間に合わない。`sync_spread_mode_from_reading_direction` はお気に入りの既定も変える | EPUB は先出しを使わない。`SpreadMode::with_reading_direction` を使う (§4.3) |
| T7 [P2] | 非同期列挙の合流キーが解決前に決まる。`render_page_async` が別経路 | 解決・登録・組み立てを背景スレッドで一体に。`render_page_async` は削除 (§3.1, §3.3) |
| T8 [P2] | 秒精度の (mtime, size) は衝突し得る | EPUB のスタンプは再利用しない世代 ID (§3.4) |
| T9 [P2] | 即時削除の経路 (`lookup` / `clear_all`) とクラッシュ時の未登録世代 | EPUB 行は全経路で削除予約。削除ゲートで未参照ファイルと `.part` を回収、ルート配下を検証 (§4.4) |
| T10 [P3] | `.part` の検証入口が曖昧 | 変換専用の物理パス検証関数 (§3.3) |

### 第 4 回 (2026-09-25、同じ担当・同じ会話の継続)

判定: 焦点を絞った改訂がもう一度必要。基本方針は維持できる。

| # | 指摘 | 対応 (第 5 版) |
| --- | --- | --- |
| U1 [P1] | 元 EPUB の書き換えは開いている PDF の書き換えと同等ではない (未描画ページが `NotConverted`) | I7 を論理パス単位の固定に変更。実行中は元 EPUB の変更を見ず、次回起動時に反映 (§2-6, §3.2) |
| U2 [P1] | 削除前に一度も解決していない本が、その実行中に開けなくなる | I8: 削除予約中の行も次の削除ゲートまで解決に使う (§3.2, §3.3) |
| U3 [P1] | 元 EPUB の状態キーが秒精度で、同秒・同サイズの書き換えを見逃す | 完全精度の `src_mtime_ns` 列を追加 (§4.4) |
| U4 [P2] | 時刻由来の世代 ID は重複し得る。公開時に既存を置換し得る | `AUTOINCREMENT` の発番表、`no_clobber` で公開 (§3.4, §4.4) |
| U5 [P2] | 変換中の元ファイル変更を検出しない | 作業フォルダへ複写し、前後の状態一致を確認してから変換 (§4.4) |
| U6 [P2] | 有効行が指す世代ファイルの消失が未定義 | 固定表へ入れる前に存在確認。無ければ `NotConverted` + 行を削除予約 (§3.3) |
| U7 [P3] | 表示遅延の見積もりに根拠が無い | 計測を S2 の完了条件に (§4.3) |
| T3 補足 | 共有ロック取得失敗を成功扱いにしない | 失敗時はその実行で EPUB の解決を無効化し理由を示す (§4.4) |

### 第 5 回 (2026-09-25、同じ担当・同じ会話の継続)

判定: まだ実装開始には早い。阻害項目は 2 つ。

| # | 指摘 | 対応 (第 6 版) |
| --- | --- | --- |
| V1 [P1] 阻害 | I7 は初回解決後の固定で、実行開始時点の固定ではない。未解決の本は差し替え後の状態を見る | 約束を「一度解決した本は実行中固定、未解決の本は解決時点で判定」に狭める (§2-6, I7)。未解決の本が変換確認に進むのは壊れた状態ではない |
| V2 [P1] 阻害 | I8 の行の寿命が §3.3 と §4.4 で矛盾 | 状態遷移を I8 に 1 つだけ書き、他の節はそれを参照 (§3.2, §3.3, §4.4) |
| V3 [P2] | 世代 ID を `image_metas` に流すと詳細表示の更新日時・サイズが壊れる | `image_metas` は元 EPUB の属性 (表示用) のまま。スタンプは照合経路だけ (§3.4) |
| V4 [P2] | 複写前後の状態一致だけでは複写中の変更を排除できない | 書き込み共有を許さないモードで 1 ハンドルから複写 (§4.4) |
| V5 [P2] | 内容同定: `from_path(x.epub)` が `.epub` を認識しない。現在の元バイト列をハッシュする | 複写時に SHA-256 を計算して行に保存し、EPUB の内容同定はその値を使う。`ContentKind` に EPUB を足す (§4.1) |
| V6 [P3] | 旧版の説明が残る。S2 の完了条件に I8 が無い | 修正 (§3.4 の表、§5) |
| 補足 | `src_mtime` のナノ秒表現の範囲、読み取り専用接続とスキーマ移行の順序、共有ロック前の変換・キャッシュ操作 | FILETIME 100ns の 1 列、ゲートで移行してから受付、共有ロック成立まで開始しない (§4.4) |

### 第 6 回 (2026-09-25、同じ担当・同じ会話の継続)

判定: まだ設計合意に至らない。阻害項目は 2 つ。核は維持できる。

| # | 指摘 | 対応 (第 7 版) |
| --- | --- | --- |
| W1 [P1] 阻害 | 内容同定の値が世代とともに残らない (有効行が置き換わると旧世代のハッシュを引けない。先頭 64 KiB のハッシュも要る。`PdfFile`/`PdfPage` からは常に `Pdf` 種別) | 不変の世代表に全値を保存し、固定表の世代 ID で引く。`GridItem` からの種別判定にも EPUB を足す (§3.2 I8, §4.1, §4.4) |
| W2 [P1] 阻害 | 複数インスタンスの同時変換で同じ元状態に 2 世代が並ぶ。削除予約後に別インスタンスが公開した世代が残る。`convert_lock` はプロセス内だけ | `BEGIN IMMEDIATE` で公開と削除予約を調停。先に公開した世代を採用、削除は削除時点の現在世代だけが対象と定義 (I8) |
| W3 [P2] | 「まだ表示していない本」と「まだ解決していない本」は違う (サムネイル・先読みで解決される) | 利用者向けの説明を「読み込んだ本 (サムネイル・先読みを含む)」に修正 (§2-6) |
| W4 [P2] | `image_metas` はキャッシュ照合 (auto-aspect の初期シード、編集プレビュー) にも使われる | 照合入口として表に追加し、EPUB は解決済みスタンプを使うか先行ヒットしない (§3.4) |
| W5 [P2] | 管理画面の `total_size()` は EPUB を含む | `epub_cache` への分離で構造的に除外 (§4.4) |
| W6 [P2] | 旧版の `clear_all()` が EPUB の世代ファイルを即時削除し得る | 旧版が知らない `epub_cache` へ分離 (§4.4) |

### 第 7 回 (2026-09-25、同じ担当・同じ会話の継続)

判定: まだ実装着手の設計合意に至らない。阻害項目は 2 つ (状態遷移の細部)。核は維持できる。W1〜W6 は解消または中心部分が解消。

| # | 指摘 | 対応 (第 8 版) |
| --- | --- | --- |
| X1 [P1] 阻害 | 世代ファイル欠落後、再変換しても現在表に残った欠落世代が「先勝ち」で再採用される | 欠落の検出で、現在表がその世代を指していれば原子的に外す。公開規則 2 は「ファイルが存在する」場合だけ先勝ち (I8) |
| X2 [P1] 阻害 | 変換中に元が変わった古い変換が、遅れて公開して新しい有効世代を上書きする | 公開トランザクション内で元 EPUB を書き込み排除で開いたまま状態を再確認し、違えば `Stale` として破棄 (I8 規則 1) |
| X3 [P2] | 世代 ID の発番 (行挿入時) とファイル公開 (先) の順序が実装できない | 変換前に発番表で予約し、欠番は再利用しない (§4.4) |
| X4 [P2] | 未固定の本の内容同定 (台帳 backfill) の取り方が無い。§3.1 と §4.1 が食い違う。台帳は Unix ナノ秒 | 固定済みは世代表、未固定は元 EPUB から計算 (PDF と同じ)。§3.1 を修正。時刻は Unix ナノ秒で統一 (§3.1, §4.1, §4.4) |

### 第 8 回 (2026-09-25、同じ担当・同じ会話の継続)

判定: **第 8 版は実装開始可能**。X1〜X4 は解消。残件は公開・所有モデルの再設計を要さず、実装とテストへ折り込める。

| # | 指摘 | 対応 (第 8 版に反映) |
| --- | --- | --- |
| Y1 [P2] | `epub_cache/` を丸ごと消すと発番 DB も消え、世代 ID が 1 から再利用されて残った派生データと衝突し得る | DB を `<data_dir>/epub_cache.db` (PDF ディレクトリの外) に置く (§4.4) |
| Y2 [P2] | `i64` の Unix ナノ秒は FILETIME の全範囲を表せず、範囲外が飽和して変更検出に使えない | 世代表は FILETIME ticks。台帳へは明示変換 (§4.1, §4.4) |
| Y3 [P2] | 未固定 backfill と固定後の編集記録の順序が未規定 | backfill 要求に来歴を持たせ、台帳の書き込み直前に照合 (§4.1) |
| Y4 [P3] | `Stale`・負けた世代の行とファイルの後始末が曖昧 | どの結果でもまず世代表へ挿入してから削除予約 (I8) |
| 助言 | `BEGIN IMMEDIATE` の保持時間 | 元 EPUB のハンドルはトランザクションの前に取る (I8) |

### 利用者判断 (2026-09-25、第 8 版の合意後)

- D9: 実行中の元 EPUB の差し替えは、一度読み込んだ本では次回起動まで反映しない、で了承。
- D10: 綴じ方向の自動適用は「PDF / EPUB の右開き指定に従う」設定 (既定 OFF) とする。利用者の指摘: 右開きでは左右キーの
  進行方向が変わるので、自動では困る利用者もいる。既定 OFF なら従来の動作を壊さない。§4.3 を改訂。


## 9. 段階の記録

### S3a 一覧・分類・D5 (2026-09-26)

EPUB の論理パスを `PdfFile` / `PdfPage` に保持する。一覧・検索・評価・コレクションが
「ページを持つ本か」を問う場合は `folder_tree::is_paged_document_path` を使う。
`ArchiveFormat` / `from_extension` / `CONVERTIBLE` は変更しない。S3a に共通の変換入力型は
不要だったため `ConvertSource` は S3b へ送る。同名 PDF 優先は既定 ON で、候補集合内の
stem を大文字小文字を区別せず比較する。設定は `settings_kv` の通常 bool であり、
公開済み DB の schema を変えない。Remote の閲覧判定は S4 で扱う。

分類監査表。判定 (a) はページを持つ本、(b) は実 PDF 専用、(c) は Remote / S4、
(d) は別の問いまたは既に EPUB 対応済み。行番号は S3a の実装時点。テスト欄の「共通」は
同じ `is_paged_document_path` の実経路を通るテストを指す。UI からの重複 reload 回避の
ように分岐単体をテストしても状態の新しい不変条件が増えない箇所は、その理由を記す。

| 箇所 | 実際の問い | 判定・検証 |
| --- | --- | --- |
| `folder_tree.rs:174,189` | 論理パスがページ本か、仮想フォルダーか | (a) 共通述語。`paged_document_navigation_skips_same_name_epub_with_pdf` |
| `folder_tree.rs:503` | DFS で本ファイルへ立ち寄るか | (a) 同上の `folder_should_stop` assertion |
| `app/folder_scan.rs:584` | 物理フォルダーのファイルを本タイルにするか | (a) `epub_is_paged_grid_item_and_same_name_pdf_wins_only_when_enabled` |
| `app/smart_folder.rs:3880` | スマートフォルダーの候補を本にするか | (a) `scan_applies_container_and_image_duplicate_settings_per_directory` |
| `app/subfolder_expansion.rs:741` | 再帰結果を本 1 項目にするか | (a) `zip_and_pdf_are_each_listed_once_without_enumerating_their_contents` |
| `collection_store/prepare.rs:692` | コレクションの実ファイルを本にするか | (a) `classifier_preserves_sources_and_distinguishes_missing_and_unsupported` |
| `tag_view.rs:488` | タグ結果の実ファイルを本にするか | (a) `tag_view_classifies_audio_without_folder_fallback` |
| `rating_view.rs:450,486` | 評価済み本 / ページキーを復元するか | (a) `restores_explicit_pdf_page_without_index_conversion` |
| `app.rs:57078,57103,79747` | ★の本 / ページとコンテナ評価の種別を復元するか | (a) `epub_rating_keys_use_pdf_book_and_page_routes`。履歴は同じ `PdfFile` variant からキーを得るので別の拡張子判定は無い |
| `app.rs:56670` | 現在の一覧がページ列か | (a) `grid_container_kind_checks_follow_what_is_open` |
| `app.rs:6250` | スマートフォルダーから開く子の種別は本か | (a) 既存 `epub_valid_smart_pdf_child_published_restarts_pdf_preflight` の owner 経路 |
| `app.rs:22173` | 仮想フォルダーを開いたとき自動全画面へ進むか | (a) `is_virtual_folder` の共通述語を使用。`paged_document_navigation_skips_same_name_epub_with_pdf` で仮想フォルダー分類を検証。全画面遷移は UI 結合状態なので単体テストを増やさない |
| `app.rs:21437` | ソート変更でページ列の物理フォルダー再走査を避けるか | (a) 共通述語。別の単体テストは reload の UI 状態を複製するだけなので追加しない |
| `app.rs:22489,22708` | 開く要求が PDF 系の列挙と履歴 snapshot を使うか | (a) `unconverted_epub_grid_open_routes_to_owned_conversion_dialog`、既存 `epub_openable_path_uses_the_pdf_enumeration_route` |
| `app/cache_ops.rs:421` | フォルダー代表の候補はページ本か | (a) 共通述語の再利用。候補選別は非同期バッチ worker 内の単純分岐で、独立した単体テストは述語だけの再検査になる。世代境界は既存 `epub_batch_parent_row_requires_generation_while_pdf_keeps_presence_check` と S2c-2 の代表ピンテストで検証 |
| `thumb_loader.rs:565,3212,3288` | 本のサムネイルキャッシュ・ピンの候補か | (a) `cache_decision_auto_webp_always_caches`、`unconverted_epub_tile_uses_icon_fallback_without_recording_failure`、既存の世代スタンプ / ピン test 群 |
| `global_search_ui.rs:672,1158` | Ctrl+G の代表 / 行はページ本か | (a) `build_drilled_items_classifies_pdf_zip_and_image_by_extension`、`build_flat_items_sorts_classifies_and_skips_zip` |
| `name_bulk_indexer.rs:362` | Ctrl+S の本名索引候補か | (a) `bulk_collects_folders_zips_pdfs_and_ignores_other_files` |
| `search_walker.rs:313` | Ctrl+G の初回走査候補か | (a) `new_files_go_to_ingest` (EPUB を追加した混在 fixture) |
| `indexer_supervisor.rs:928` | Ctrl+G の差分更新候補か | (a) `build_candidate_from_path_rejects_zip` の EPUB assertion |
| `metadata_transfer.rs:1422` | 持ち運び用メタデータの本 / ページ種別か | (a) `epub_transfer_uses_paged_book_metadata_kind` |
| `snapshot.rs:238` | フルスクリーンの擬似パスを本 / ZIP の所有者と内側ページへ分けるか | (a) 末尾の `p:N` の直前だけを `is_paged_document_path` で判定。ZIP / CBZ は従来どおり最も外側。`split_archive_path_uses_the_book_immediately_before_the_page`、実ナビ `snapshot_pdf_pages_inside_epub_named_folder_keep_their_owner` |
| `app/snapshot_ops.rs:1206,1231,2132` | 現在の PDF / EPUB ページから★固定リストの所有行を得るか | (a) `PdfPage` の擬似パスは Windows の `p:` ドライブ解釈を避けて構築し、上記の実ナビテストで検証 |
| `global_search_ui.rs:647` / `zip_loader.rs:964` | ZIP hit の専用区切り / ネスト ZIP entry の区切りか | (d) 前者は U+001F、後者は ZIP / CBZ の内側専用。PDF / EPUB の擬似パスは受け取らないため変更不要 |
| `ingest_worker.rs:347` / `app/metadata_ops.rs:1959` | PDF Info と EPUB の内部タイトル・著者を検索するか | (d) EPUB は元ファイル名だけ。`ingest_converted_epub_uses_filename_without_generated_pdf_metadata`、`current_folder_filter_searches_epub_name_but_not_generated_pdf_info` |
| `ui_dialogs/preferences/pages.rs:1109` / `preferences.rs:1189` | 選択中の本に対応する外部アプリの拡張子を初期選択できるか | (d) EPUB を候補へ追加。`preferences_selects_epub_association_from_selected_book` |
| `ui_main.rs:4614,4629` | 詳細欄に何の形式と表示するか | (d) EPUB / EPUB ページを個別表示。`shared_builder_formats_zip_and_pdf_container_fields` |
| `app/grid_paint.rs:211` | タイルの形式バッジは何か | (d) EPUB バッジ、PDF と同じアイコン・描画。`archive_types_always_show_a_format_badge` |
| `app.rs:26733,26836,27502` | PDF 専用の保存 / セッションパスワードと入力ダイアログか | (b) EPUB を除外。`the_saved_password_wins_over_the_password_of_the_pdf_that_happens_to_be_open` の EPUB assertion、既存の typed failure tests |
| `delete_worker.rs:444,464` | 削除前に PDF パスワードのハッシュキーを集めるか | (b) EPUB には PDF パスワードが無い。既存 `collect_pdf_paths_for_delete` test |
| `rename_key_migration.rs:2305` | 単一 PDF 改名で PDF パスワードを移すか | (b) EPUB は別の同定 / 世代。既存 rename migration tests |
| `catalog.rs:1296` | 旧 PDF layout 寸法の行を移行するか | (b) PDF 旧 catalog 専用。EPUB 世代行は別 stamp。既存 catalog migration tests |
| `app.rs:27737,27743,27773` | 旧 PDF の親サムネ seed と writeback を使うか | (b) EPUB は冒頭で除外し、100 ms の prefetch grace のみ共有。既存 `epub_virtual_folder_seed_and_parent_writeback_are_explicitly_excluded` |
| `similar_index.rs:5704` | 旧 mtime/size による PDF 類似索引を生成できるか | (d) 世代 stamp の無い類似索引に EPUB を入れると旧ページを有効扱いする。S2c-2 からの意図的除外を維持 |
| `content_identity.rs:115,796` | 台帳に保存する**元ファイル**種別は何か | (d) EPUB は `ContentKind::Epub` として S2c-2 で実装済み。既存 content identity tests |
| `app.rs:22493,22713` (旧行) | 開く対象がページ本か | (d) S2c-1 から PDF / EPUB とも通過。S3a で共通述語へ移動し、上記の現行行へ統合 |
| `bin/bench_scroll.rs:137,165`、`bin/bench_dupe.rs:1808` | PDF 専用の診断ベンチ入力か | (d) 製品の一覧 / 操作経路ではないため既存対象を維持 |
| `remote_ipc/container.rs` の PDF ページ / subresource / location 判定 | 論理パスがページ本か | (c) S4 完了。`is_paged_document_path` を使い、実 PDF のパスワードだけ PDF 専用。EPUB の本・ページの Remote address は元 `.epub`。open/page/表紙の操作ごとに世代を固定し、見開き個別寸法も stamp を照合。D10 は EPUB と通常 PDF に適用。Remote 一覧・ページ数・未変換・綴じ方向・アーカイブ先行分岐・世代競合・実 PageRequest のテスト |
| `remote_ipc/thumbnail.rs` のコンテナ判定 | Remote の本の表紙を本体で生成するか | (c) S4 完了。EPUB も本体のページ 0 を要求し、未変換なら Web の EPUB プレースホルダー。Remote 表紙と Web タイルのテスト |
| `crates/remote-web/src/store.rs`、`web/app.js` の拡張子再分類 | Remote が受け取る本種別と遷移先か | (c) S4 完了。既存の `pdf` wire kind で EPUB を検証・表示し、EPUB バッジと PDF 本の遷移先を使用。store / JS の回帰テスト |

`PdfFile` variant を直接見るレーティング、タグ、コレクション、履歴、しおり、代表ピンの
消費側は追加分岐が不要。上記の分類入口と、S2c-2 の論理パス / 世代スタンプの既存テストを
通る。Remote 以外のファイルを stat する処理は worker 側に置き、フォルダー走査の
ファイル / ディレクトリ判定は引き続き `entry.file_type()` を使う。

回帰テストの負例は、共通述語を一時的に PDF 専用へ戻して分類・検索・評価・開く経路を、
D5 の通常一覧 / ツリーのフィルタを無効化して重複表示を、それぞれ検出した。
サムネイルの worker 解決、世代スタンプ、EPUB バッジ、詳細欄、PDF パスワード境界、
設定 DB の読み出し、環境設定 snapshot も各々の処理を一時的に戻して失敗を確認した。
差し替えは各実行後に元のバイト列へ復元した。

#### S3a 独立レビュー修正 1

★固定した EPUB ページ一覧では `snapshot_current_fullscreen_path` が `<book.epub>/p:<num>`
を作り、`snapshot_owner_entry` がそのページを探す。旧 `split_archive_path` は `.epub/` を認識せず、
さらに Windows の `PathBuf::push("p:1")` は `p:` をドライブとして扱って本のパスを失っていた。
両方を直し、擬似パスの分割は本 / ZIP の component の拡張子で型判定する。同類の splitter は
上表の ZIP 専用 2 箇所だけで、EPUB ページは受け取らない。

EPUB の検索語は元ファイル名のみ、結果の同定パスは元 EPUB。内部のタイトル・著者や変換後 PDF の
Info 辞書は索引も Ctrl+F も対象外にする。取込は EPUB を先に明示分岐し、未変換を失敗ログにしない。
PDF の Info 読取分岐は維持する。外部アプリの関連付け候補には `.epub` を追加した。

負例では対象条件を一時的に旧動作へ戻し、各テストが compile error ではなく assertion で失敗した。
`split_archive_path` を PDF 専用にすると分割単体と EPUB ページの実ナビが失敗し、別に
`PathBuf::push("p:1")` を戻しても実ナビが失敗した。取込と Ctrl+F の EPUB 専用分岐をそれぞれ
無効にすると、生成 PDF にだけあるタイトルがヒットして失敗した。関連付け候補から EPUB を外すと
初期値が JPG に戻って失敗した。差し替えたファイルはすべて元のバイト列へ復元した。

#### S3a 独立レビュー修正 2

`.epub` / `.pdf` で終わる実フォルダー内の本では、左側から最初に見つけた拡張子は
本の境界ではない。`p:N` を右端で確認し、その直前の component だけを PDF / EPUB の本とする。
`shelf.epub/book.pdf/p:1`、`shelf.pdf/book.epub/p:1`、`book.epub/p:1` を分割テストで検証し、
最初の経路は★固定ページ一覧の実ナビテストでも検証した。最初の経路の両テストは
`1c36286b3` の分割実装へ戻すと所有者の assertion で失敗した。ZIP / CBZ 内画像は最も外側で
分ける従来の規則を維持する。

公開済み検索ストアの互換性判断: **`fts_meta::INDEX_VERSION` は 10 のまま**。
Ctrl+S は従来の `PdfFile`、Ctrl+G は従来の `Pdf` kind で EPUB の新規行だけを追加し、
既存行・schema は変えないため移行も全件再構築も不要。v4.1.0 のコードで戻り動作を確認した。
Ctrl+S に残った EPUB 行は本タイルになるが、開くと旧 `.pdf` 限定判定から通常ディレクトリ走査へ進み、
読取失敗を表示する。Ctrl+G の EPUB 行は旧拡張子分類で画像タイルになり、クリックすると画像読取に
失敗する。**その失敗より前に**旧 `open_fullscreen` が `record_book_resume` を呼び、検索元フォルダーの
読書位置を `book_resume.db` に上書きし得る。旧版の索引走査は通常 EPUB を候補から外して行を削除する (Susie プラグインが `.epub` を画像拡張子として申告する構成では残り得る)。
現行版に戻すと次の走査で EPUB を候補に含め、欠けた Ctrl+G 行は `search_walker::scan` →
`IngestSession::apply`、Ctrl+S 行は `name_bulk_indexer` が再登録する。版上げ無しで復旧することを
現行コードと既存の `new_files_go_to_ingest` / 名前索引の EPUB テストで確認した。
全利用者へ強制再構築を課すより稀なダウングレード時のこの制約を受け入れる。詳細は
`docs/search-architecture.md` §3。

### S1 変換器の仕上げ (2026-09-25、**独立レビュー 4 回目で承認**)

実装: Codex Sol (同一セッションで 4 往復)。実機の確認は検収側 (ClaudeCode) がサンドボックス外で実施 (Codex のサンドボックス内では
WebView2 が起動しない)。独立レビュー: 別の Codex Sol セッション (読み取り専用)、4 回目で承認。最終コミット `78b91108c`。

| 完了条件 (§5 S1) | 結果 | 確かめ方 |
| --- | --- | --- |
| 通信ゼロ | 合格 (DNS は仕様に基づく) | `127.0.0.1` に **TCP の accept を記録するリスナー**と HTTP サーバーを立て、preconnect / dns-prefetch / prefetch / preload / modulepreload と外部の CSS・フォント・画像・iframe・背景画像を含む合成 EPUB (リフロー・固定) を変換。TCP accept **0 件**・HTTP **0 件**。要求フィルタは全 source kind (`ICoreWebView2_22`)。DNS は管理者権限なしでは直接観測できないため、Chromium の `--host-resolver-rules="MAP * ^NOTFOUND"` (`net/dns/mapped_host_resolver.h` で構文を確認) による |
| 外部設定による上書きへの耐性 | 合格 (ポリシーは D11 で対象外に変更) | `WEBVIEW2_USER_DATA_FOLDER` と `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--host-resolver-rules=` を与えても、指定した一時フォルダを使い通信 0 件。レジストリのポリシー (AUMID → exe 名 → `*`、HKLM/HKCU、両ビュー) の検出は、利用者設定を書き換えないため偽レジストリの単体テストで確認。実際の一時フォルダはファイル ID で照合 |
| 本のスクリプトを実行しない | 合格 | 合成 EPUB の inline script が動かない (`book_script_ran=false`、script の `fetch` も発生せず)。ホスト側の判定は CDP |
| Job Object (中止・親の強制終了) | 合格 | `epub-pdf-job-probe`: WebView2 の子プロセスをコマンドラインの一時フォルダ名 (目印) でシステム全体から探す。中止直前に変換器と生存中の目印プロセス (6 個) がすべて Job 内、Job の外 0 個、終了後 約 50 ms で全消滅 |
| 27 冊の回帰 | 合格 | 固定レイアウトの 17 冊はページ数・画像判定 (JPEG の SHA-256 一致) とも不変。リフローを含む本は `reflow-v1` でページ数が増えた (想定どおり)。レビュー対応の各修正後も 27 冊で差分なし |
| リフロー `reflow-v1` (720×1024 CSS px、余白 32 px) | 採用 | 草枕 (本の CSS どおり 1 行 28 字)、Moby-Dick、ごん狐を画像化して目視 |

**訂正の記録**: 最初の検収では「HTTP サーバーが受けた要求 0 件」をもって通信ゼロとしたが、独立レビュー 1 回目の指摘どおり、
preconnect は HTTP を送らずに TCP 接続だけを張る。修正前の変換器で TCP accept が 2 件あることを実測で確認し (確認方法が漏れを
検出できることの確認でもある)、確認方法を TCP accept の記録へ改めた。また DNS 遮断の指定は当初 `~NOTFOUND` で誤っていた
(レビュー 2 回目、Chromium のソースで確認して `^NOTFOUND` へ)。

独立レビューで見つかり直した主な点: 外部参照を含む本を不正扱いにしていた (検収側で発見)、別ボリュームへの出力公開の失敗、
進捗 JSON の最終行が出ない経路、一時フォルダの指定が出力を含むと削除される、`\\host` の外部判定、無期限待機、旧 Runtime での
部分フィルタ、外部設定による上書き、Job probe が誤って合格し得る判定、panic 後の一時フォルダ残り。

S2 へ引き継ぐ事項: 強制終了で残る `<out>.tmp-<pid>-<n>` と `.part` は本体の削除ゲートで回収する (§4.4)。本体は変換器を起動する
ときに `WEBVIEW2_*` を外し、存在しない一時フォルダのパスを渡す。終了コード 0/2/3/4/5/6/7/8 と `--progress-json` の契約は
`crates/epub-pdf-worker/README.md` と `src/protocol.rs` が正本。

合成 EPUB の生成: `C:\home\mimageviewer_testdata_epub\gen_probe.py`、`gen_probe_preconnect.py` (リポジトリ外)。

### S2a EPUB キャッシュ・起動ゲート・ホスト側ランナー (2026-09-25)

`src/epub_cache.rs` に専用 DB の世代予約、不変世代/現在/削除予約表、I8 の公開・削除予約・欠落切り離し、`.alive` を使う起動時削除ゲートを追加した。`src/lib.rs` は単一インスタンス取得後、コレクション・Remote・App の作成前にゲートを呼び、共有ロックの guard を `run` の間保持する。`src/epub_convert.rs` は固定した入力コピー、内容同定ハッシュ、Job Object 下の suspended worker、進捗 JSON、cancel/timeout、最小 PDF 確認と上書きなしの世代公開を担当する。`materializer` の死んだ PID の一時フォルダ掃除に `epub-<pid>-*` を追加した。

この段階では変換 API の UI 接続、PDFium の `verify_converted_pdf`、`pdf_loader` の解決と固定表、グリッド・管理 UI は未実装 (S2b / S2c / S3)。実 WebView2 ワーカーの起動とネットワーク・Job 子孫の再実測は隔離環境外の検収で行う。
`epub_cache.db` は新設かつ未リリースなので、既存データ用のスキーマ移行は設けない。
進捗 JSON の serde 型はワーカーが独立 workspace package である構成を保つため、本体に同じフィールド名・列挙値を小さく複製する。S2a の fake worker テストには Windows の実 `CreateProcessW` / Job Object 経路を通るものを含める。
独立レビューの追補では、`.alive` の共有モードから delete を除き、保持中の名前変更を防いだ。起動時掃除は `generation_ids` に記録した予定 PDF パスと完了状態を使い、retired 行と未完了予約だけを処理する。削除対象は再解析ポイントを辿らずに開き、ハンドル上の最終パスがキャッシュ内にあると確認して同じハンドルから削除する。全世代の走査は行わない。変換ランナーは `ConvertedPdfVerifier` の注入を必須とし、S2a の最小 PDF 確認はテスト専用とした。S2b の UI 接続には PDFium による検証器の実装が必要。

### D11 の反映 (2026-09-25)

S1 の独立レビューはポリシーによる上書きの検出を求め (2 回目・3 回目)、変換器は終了コード 7 で拒否する実装になっていた。
利用者判断 D11 により検出を取りやめ、ポリシーに従う。変換器からポリシー検出と終了コード 7 を削除し、一時データの保存先の
不一致は失敗にせずレポートに記録する (`user_data_folder_redirected`)。本体 (`epub_convert.rs`) からも `WebView2Overridden` を
削除し、7 は想定外の終了コードとして扱う。`WEBVIEW2_*` 環境変数の除去は変換器・本体とも継続。反映後にサンドボックス外で
通信 (TCP 0・HTTP 0)、環境変数上書きへの耐性、一時フォルダの削除を再確認した。

### S2b pdf_loader 読み取り境界 (2026-09-25)

`pdf_loader` に EPUB 論理パスから不変 PDF 世代への解決とプロセス内の挿入専用固定表を追加した。
初回だけ元ファイルの FILETIME・サイズを現在世代と照合し、欠落世代は条件付きで切り離す。
起動ゲートの結果は `pdf_loader` が保持する。通常 PDF は解決時の stat を行わない。
同期の PDFium 入口と非同期列挙は実読込パスを IPC に渡し、列挙合流も実読込パスで行う。
非同期列挙は解決・合流登録・要求組み立てを背景スレッドへ移し、型付き失敗を全 waiter に配布する。
未使用の in-process `render_page_async` を削除した。列挙応答に方向を追加し、
`PdfiumConvertedPdfVerifier` で物理 `.part` を検証する。
EPUB を UI から開く導線、世代スタンプの全派生キャッシュへの接続、方向設定と適用は S2c/S3 に残る。
レビューで S2a の `src_key` がドライブ文字を落とし別ドライブの同名 EPUB を衝突させることが判明したため、
未リリースの EPUB 専用 DB・世代ファイル名・固定表を `normalize_keep_drive` に統一した。
表示用のページ属性は初回解決時の元 EPUB 状態で固定し、列挙結果の世代スタンプとは分離した。
通常 PDF の列挙 admission は呼出元で I/O 無しに待ち手を登録し、EPUB の DB 待ちとは分離する。
設計オーナーの追補決定により `pdfium-render` のローカル fork は採用しない。EPUB 方向は固定した
世代行の `direction` から渡し、変換 PDF を方向のために再読込しない。通常 PDF は `want_direction=true`
のときだけ公開 PDFium bindings の `FPDF_LoadDocument` / `FPDF_VIEWERREF_GetName` /
`FPDF_CloseDocument` で追加 open する。既定 `false` には追加 open が無い。
通常 PDF の列挙合流キーには `want_direction` を含め、方向不要の in-flight 応答へ方向要求を合流させない。
EPUB は常に世代行の方向を応答へ載せるため、ワーカーへは `want_direction=false` を送る。
追加 open の失敗は方向無しとしてページ列挙を維持し、ワーカーの診断へ理由を記録する。
この条件付き取得は §4.3 の常時方向取得という旧記述に優先する。
S2c ではスマートフォルダーの `PdfPages` 経路にも列挙結果の方向を運ぶ。
独立レビュー追補: App の新旧ハンドル handoff で旧要求が先に取消されないよう、通常 PDF と固定済み
EPUB は新しい待ち手を返す前に同期登録する。未固定 EPUB の解決・登録は引き続き背景で行う。
固定表の mutex はメモリ検索・挿入だけを保護し、元ファイル stat・DB 照合中は保持しない。`ResolvedReadPath` を要求エンコーダの必須型とし、
pool 投入では `PdfPoolRequest` がエンコード済み要求と解決済みパスを対にして解決漏れをコンパイル時に防ぐ。
通常 PDF の無 stat 判定は純粋な拡張子分類で検証する。

独立レビューの P2 追補: 変換 PDF の物理検証は `EpubCache::reserve_output` が予約を確定したときだけ発行する
`ReservedOutput` token を要求する。token は非公開フィールドに世代 ID・世代 PDF パス・予約済み `.part` パスを持ち、
変換ランナー、`ConvertedPdfVerifier`、`pdf_loader::verify_converted_pdf` の順にそのまま渡す。
裸の `Path` を物理例外として PDF pool に投入できない。通常 PDF の解決は EPUB 側の I/O 分岐を注入したテストでも
呼出し 0 回を確認する。

### S2c-1 オープン導線・変換ダイアログ・D10 (2026-09-25)

アドレスバー・起動引数・復元先の `.epub` を PDF 系のオープン経路へ送る。
PDF 列挙の型付き失敗を通常オープン、コレクション、スマートフォルダーで受け取り、未変換は同じ要求 owner の変換へ引き継ぐ。
事前確認と変換は取消可能な背景 worker、完了時は EPUB 論理パスを再度開く。
ダイアログは既存アーカイブ変換の相を踏襲した EPUB 専用 state とし、ZIP 用の同期完了分岐や source override を通さない。
D10 は既定 OFF で追加し、ON かつ見開き保存値が無い場合にだけ列挙の方向を既定モードへ適用する。
変換制限時間は 600 秒。グリッド分類、派生データスタンプ、同名スキップは後続段階へ残す。

### S2c-1 修正 1 (2026-09-25)

D10 は本ごとのページ構成と読み方向がともに未保存のときだけ文書方向を両方へ適用する。
Single は Single のまま読み方向を保持する。ページ構成のみ保存されていればその向きを読み方向へ、
読み方向のみ保存されていれば既定の見開きモードをその向きへ写す。
既定 OFF の PDF オープンは spread.db を追加照会しない。
EPUB 変換状態は PDF 列挙 pending と同じ viewer context bundle に所属させ、context pause で取消す。
公開・採用の遅延結果は再オープン前に要求 owner を検証し、古い結果は表示へ適用しない。

### S2c-1 修正 2 (2026-09-25)

スマートフォルダーの EPUB 変換要求は PDF 子と `EpubConvert` 待機相を持つ要求 ID で検証する。
変換結果の公開後は同じ PDF 子の事前列挙を再開し、古い要求は表示に適用しない。
変換要求には viewer context の既存画面世代とスマートフォルダー要求世代を記録し、
コレクション root など別種のナビゲーションで世代が変わればダイアログを閉じて遅延結果を捨てる。
スマートフォルダー要求世代は main context の変換だけに照合し、別 viewer context の要求には波及させない。
取消・置換・context pause は共通の終了処理を通し、スマートフォルダーの `EpubConvert` 待機も解除する。
残件: フォルダペインの先行スキャンは画面採用前に始まり、上記二つの世代を進めない。
この入力時点まで「任意の後続オープン要求」を保証するには、viewer context ごとの共通要求世代と、
detached 宛てスキャンとの所有境界を確定する必要がある。凍結中の detached 経路を局所的に分岐させず、
設計確認後に扱う。

### S2c-1 修正 3 (2026-09-26)

EPUB 変換の終了を `Abort` と `Superseded` に分けた。利用者による取消や context pause は
旧要求の履歴・アドレス・遅延 fullscreen を失敗として戻す。一方、後続オープンが画面を
所有した場合は、旧要求の rollback と遅延 fullscreen を破棄し、後続オープンの履歴・
アドレスを変更しない。フォルダペインの `PaneNavigation` は main context の独立した
入力と確定しているため、worker 走査の開始時点で旧 EPUB を supersede する。
`GridFolderCandidate`、detached 対象、fullscreen 目的、表示順 refresh は同じ所有者を
確定できないため、この境界では退役させない。上記の残件はこれで解消した。

変換 worker の印刷進捗は PDF ページ数を分母とせず、印刷対象の linear spine 項目の
完了数 / 総数で示す。固定レイアウトの chunk は含む項目数だけ加算する。印刷済み PDF
ページ数は進捗 JSON の `pages` に分離し、ダイアログの文言へ反映する。旧形式の
`pages` が無い進捗イベントも runner は受け付ける。

### S2c-1 修正 4 (2026-09-26)

EPUB 要求の終了では、取消・置換・後続ナビゲーションの別を問わず worker を取消し、
旧要求が保持した遅延 fullscreen のナビゲーションロックを既存の解放経路で終える。
履歴 snapshot とアドレスは画面の復元情報として別に扱う。取消時は復元し、
ペイン先行走査への置換時は走査 pending と完了候補へ渡す。走査が失敗すれば復元し、
フォルダを採用できたときに破棄する。EPUB から EPUB への置換でも旧ロックは引き継がない。
置換先の EPUB を取り消した場合は、置換先のアドレスが入力済みでも最初の閲覧位置へ戻す。
公開後の PDF 列挙だけが遅延 fullscreen を明示的に引き継ぐ。
印刷進捗テストは spine 比率と印刷済みページ数をそれぞれ直接検証する。

### S2c-1 修正 5 (2026-09-26)

ペイン先行走査の pending と ready は EPUB の履歴・アドレス復元情報を一つだけ所有する。
同じフレームで走査 B が完了しペイン C が選ばれても、復元情報を ready B から pending C へ渡す。
別のペイン走査への置換では渡し、採用確定時に破棄し、走査失敗・取消・context pause では
EPUB の中断と同じ復元を行う。通常フォルダ load の走査取消も、採用または失敗が確定する
境界へ移した。EPUB 間の置換では最初の要求の履歴 snapshot を次の要求に渡し、次を取り消すと
最初の要求より前の「戻る」先へ復帰する。

### S2c-1 検収 (2026-09-26)

独立レビュー 7 回目で承認 (`5d8ead73e`)。1〜6 回目の指摘は修正 1〜5 で対応した。
6 回目の指摘「ペイン走査待ち中に一覧でスマート子・変換対象アーカイブを選ぶと、後から古い走査が採用され得る」は
master に既存の不具合 (両経路とも `load_folder_with_scan_claimed` の取消を通らず、フレームの優先判定にも一覧
操作が無い) と確認し、EPUB とは別の課題として切り出した。実機: 利用者がアドレスバーからの EPUB 変換・閲覧・
取消とボタン配置を確認。設計担当が moby-dick.epub で進捗 (142 項目・464 ページ・約 30 秒) を確認。

### S2c-2 派生データ・鮮度判定の全経路調査 (2026-09-26、実装前)

以下は §3.4 の表を出発点に、`mtime`/`file_size`、`image_metas`、
`pdf_meta`、ピン、バッチ生成、スマートフォルダ、内容同定、類似索引まで
読み書きの**項目**を照合した一覧。EPUB の `stamp` は世代 ID と世代 PDF サイズの
整数ペアを指す。表示専用の `image_metas` は元 EPUB の属性を保つ。

| 派生データ / 処理 | 現在の照合・保存点 | EPUB に必要な判定と変更 |
| --- | --- | --- |
| 親一覧の `pdf_meta` placeholder | `app.rs:27039-27115` は論理ファイルを stat し、warm catalog の行と比較 | 固定表に既にある世代の stamp だけで照合。未固定は省略。D10 の方向待ちを優先。通常 PDF は従来の stat を維持 |
| スマート子の warm placeholder | `app/smart_folder.rs:3005-3037` が上記 peek を呼ぶ | 同じ判定を共有。固定済みだけ表示 |
| 列挙後の親 `pdf_meta` 保存 | `app.rs:27268-27302` は列挙ページ 0 の属性を使用 | `PdfEnumerateResult.stamp` から世代ペアを保存。`pages` の元 EPUB 属性は表示専用 |
| `pdf_meta` の catalog 行 | `catalog.rs:901-1024` が `(mtime,file_size)` で比較・更新 | スキーマを変えず、EPUB 行の整数ペアに世代 stamp を格納 |
| サムネイルの UI 要求 | `app.rs:79861-80142` の `make_load_request` は `image_metas` とピン stat を転記 | EPUB の `PdfFile` / `PdfPage` と EPUB ページを指すピンは worker 解決印を持つ。元属性を cache hit に使わない |
| サムネイルのメモリ hit | `thumb_loader.rs:1219-1231` は要求の属性と `cache_map` を比較 | EPUB は同じ `ReadTarget` の stamp で比較 |
| サムネイルの生成・catalog 保存 | `thumb_loader.rs:1725-1755,3101-3245` は要求属性で保存、render は再解決 | EPUB は照合済みの target で render して同じ stamp を保存 |
| サムネイル catalog 行 | `catalog.rs:21-33,692` の `mtime,file_size` | スキーマを変えず EPUB 行を世代ペアで保存。PDF ページと親タイルの両方 |
| サムネ cache hit 後の `pdf_meta` catch-up | `thumb_loader.rs:2229-2270,2025-2097` は要求属性または論理 stat を用いる | worker の一解決 stamp で hit、列挙、保存 |
| 隣接本の先読み | `thumb_loader.rs:2110-2224` は論理 stat と親 WebP / `pdf_meta` の属性比較 | 一解決 stamp で両行を照合し、同じ target で render して保存 |
| バッチキャッシュ生成 | `app/cache_ops.rs:608-798` はフォルダ走査値で列挙・行比較・render・保存 | EPUB を対象にする際は一解決 stamp を両 catalog と全ページに通す。対象外にするだけでは本の一括生成が欠ける |
| 詳細欄のページ数 | `app/metadata_ops.rs:1406-1467` は走査時の属性で親 `pdf_meta` を照合 | worker で解決した stamp を使って列挙と保存まで通す |
| 編集済みページのプレビュー | `app.rs:62862-62986` が表示属性を保存要求へ渡し、`edit_preview_cache.rs:1231-1280` がコンテナ size を保存。`thumb_loader.rs:1210-1225` はコンテナ属性を比較 | 保存 worker と読込 worker の両方で世代 stamp を使用。既存整数列の EPUB 行のみ意味を変える |
| auto-aspect の最初の seed | `app.rs:18305-18405` は UI の `image_metas` / 要求属性で WebP を比較 | EPUB は UI で解決せず seed を省略し、worker サムネ結果の sample を使う |
| ピンの source ID と代表 WebP | `folder_thumb_pins.rs:640-759,814-947` は target の stat を ID に埋め込み、`app.rs:80000-80135` 等が要求キーへ写す | EPUB leaf では世代 stamp を source ID と要求の照合属性へ使う。UI での解決を避けるため worker 側で確定させる |
| フォルダ代表選定・seed | `thumb_loader.rs:2755-2805` は選定元の stat / ピン ID を使う | 通常の代表選定は worker stamp を使う。ドライブ一覧の間接ピン seed は別経路なので下記で扱う |
| ドライブ一覧の間接ピン seed | `app.rs` は子フォルダの代表行を stage 前と同じ規則で先にコピーする。`thumb_loader.rs` は暫定 seed の後、現行子ピンから作る完全一致 key と EPUB 世代を worker で検証する | UI は既存の catalog 読取だけを使い、key を解析しない。最終表示は完全一致行またはアイコン。同一 WebP・stamp・寸法ならテクスチャを再作成しない。再変換後の旧 EPUB cover は worker 到着まで暫定表示され得る。`.epub` を含む画像名は画像として扱う |
| 仮想フォルダの親子 seed / writeback | `app.rs:27709-27870` は `.pdf` / ZIP だけを対象にし、親 WebP 行を仮想 catalog へコピー、ページ 0 完成時に元ファイル stat で親へ書き戻す | EPUB は明示的に対象外。親とページ 0 が別々に描画される追加コストを受け入れる。PDFium の 100 ms 先読み抑制は EPUB にも適用 |
| 詳細の `DetailsLazyMeta` メモリ hit | `app.rs:54491-54497,5541-5545` は `image_metas` の元属性で比較 | 実行中の固定世代は不変で、このメモリ表は再起動で破棄されるため変更不要。永続 `pdf_meta` は別行で世代照合 |
| 外部渡し用 PDF ページ PNG | `materializer.rs:274-292,548-634,1202-1217` は論理ファイルの時刻とサイズを再利用判定に使う | EPUB ページは世代 stamp で再利用し、描画も同一 target。実ファイルの直接渡しは元 EPUB のまま |
| 外部渡しの結合見開き | `materializer.rs:558-566,937-951` は単一元ファイルが無く空 stamp を使い、再利用が常に miss | EPUB の左右ページも毎回合成するため stale な見開き cache hit は起きない。変更不要 |
| 内容同定台帳 | `content_identity.rs:56-77,100-155,1763-1868` は元ファイル hash、種類は PDF | `Epub` 種別を追加。固定済みは世代表のハッシュ・元サイズ・元時刻、未固定は元 EPUB。遅れた backfill は書込直前に来歴を再照合 |
| 保持ラスタ / final AI | `app.rs:65221-65261` は実行中のページ ID | I7 で世代が固定されるため追加 stamp 不要 |
| PDF ワーカー文書 cache / 列挙合流 | `pdf_loader.rs:4335-4354,5160-5210` は実読込パスで cache / 合流 | 世代ごとに物理パスが異なり既に分離。変更不要 |
| 類似画像索引 | `similar_index.rs:5712-5765,6390-6394` は `.pdf` のみ走査し元属性で再利用 | 現段階では EPUB を明示的に索引対象から除外。ページ数変更で stale EPUB 行が生じず、投入と世代 stamp は別段階で設計 |
| Remote ページ数・`PdfIdentity` | `remote_ipc/container.rs:6126-6193` は論理 stat / `pdf_meta` | S4 の対象。現段階では変更せず、Remote から EPUB を開く経路は S4 まで未対応 |

### S2c-2 実装記録

`ReadTarget::stamp` をサムネイル要求、`pdf_meta` 補完、隣接先読み、バッチ生成、詳細のページ数、外部渡し用ページへ通し、EPUB の派生行は既存の `(mtime, file_size)` 整数列へ `(generation_id, generation_pdf_size)` を保存する。通常 PDF の属性取得と照合は従来経路を維持する。UI で構築した EPUB のサムネイル要求は型付きの worker 解決印を持ち、`image_metas` は表示用の元 EPUB 属性のままにする。編集プレビューは保存 worker と読込 worker の両方で世代を解決し、auto-aspect の UI 初期 seed は EPUB のみ省略する。

上の全経路調査表から実装・テストへの対応 (行番号は S2c-2 完了時点):

| 派生データ / 経路 | 実装・変更不要の根拠 | 回帰確認 |
| --- | --- | --- |
| `pdf_meta` placeholder | `app.rs:27061,27101` は固定表 `pdf_loader.rs:214` を照合 | `epub_pdf_meta_placeholder_uses_only_a_pinned_generation` |
| スマート子の warm placeholder | `app/smart_folder.rs:3005` が同じ `peek_pdf_meta_cache` を使う | 同じ先出し判定テスト |
| 列挙後の `pdf_meta` 保存 | `app.rs:27328,76425` の worker に列挙 stamp を渡す | `epub_pdf_meta_worker_replaces_page_count_with_new_generation_stamp` |
| `pdf_meta` catalog 行 | `catalog.rs:901-1024` の既存整数列 | `epub_cache_directory_wipe_keeps_ids_and_invalidates_derived_rows` |
| UI のタイル・ページ要求 | `app.rs:80018,80030` の型付き worker 解決印 | `epub_thumbnail_requests_defer_stamp_while_pdf_requests_keep_file_attributes` |
| メモリ thumbnail hit | `thumb_loader.rs:369,1166` で先に世代ペアへ置換 | `epub_thumb_request_replaces_display_metadata_with_generation_identity` |
| thumbnail 生成・保存 | `thumb_loader.rs:1166,1725,6158` が同一 target と世代ペアを通す | 同じ要求テストと `epub_cache_directory_wipe_keeps_ids_and_invalidates_derived_rows` |
| thumbnail catalog 行 | `catalog.rs:21-33,692` の既存整数列 | 世代 ID 消去・再発番のテスト |
| `pdf_meta` catch-up | `thumb_loader.rs:2093,2182,2201` の共通世代抽出 | `epub_catchup_neighbor_and_folder_resolution_share_generation_stamp` |
| 隣接本先読み | `thumb_loader.rs:2182,2201,2215` の同一 target | 同じ共通世代抽出テスト |
| バッチキャッシュ生成 | `app/cache_ops.rs:4,609-850` の親行比較と同一 target | `epub_batch_parent_row_requires_generation_while_pdf_keeps_presence_check` |
| 詳細のページ数 | `app/metadata_ops.rs:453,1406-1475` | `epub_details_page_count_uses_generation_and_pdf_keeps_source_stamp` |
| 編集プレビュー | `edit_preview_cache.rs:1210,1231-1300` で保存、`thumb_loader.rs:1210` で照合 | `epub_preview_save_and_load_use_generation_in_existing_integer_columns` |
| auto-aspect 初期 seed | `app.rs:18381` で EPUB の元属性 hit を省略 | `epub_auto_aspect_seed_ignores_source_metadata_cache_hit` |
| ピン source ID | `folder_thumb_pins.rs:738-765` の保留印を `thumb_loader.rs:369` が確定 | `epub_page_pin_source_id_waits_for_worker_generation` と thumbnail 要求テスト |
| フォルダ代表・seed | `thumb_loader.rs:2719,2791,2950,3124` が共通世代抽出を使用 | 共通世代抽出テスト。ドライブ一覧 seed は次行 |
| ドライブ一覧の間接ピン | `app.rs` の UI seed は子 Folder 行を stage 前と同じ規則で選ぶ。`thumb_loader.rs` の cache-only worker が現行子ピンから書込側と同じ完全一致キーを作り、親 catalog を読む | seed は暫定表示。子ピン・設定深さ・EPUB 世代をキーと整数列の双方で確認した worker 結果が最終表示となる。自動代表行は通常の子フォルダ読取と共通の `folder_cached_row_usable` で選定元の状態・pin revision・依存 catalog を照合。`drive_list_indirect_epub_cover_matches_current_generation_after_reconversion`、`drive_list_child_grandchild_epub_pin_uses_configured_depth_one`、`drive_list_folder_pin_seeds_image_named_cover_epub_png`、`drive_list_missing_child_image_pin_uses_cached_auto_representative` |
| 仮想フォルダの親子 seed / writeback | `app.rs:27721` の対象分岐は PDF / ZIP のみ。EPUB は 100 ms 先読み抑制だけ適用 | `epub_virtual_folder_seed_and_parent_writeback_are_explicitly_excluded`。親とページ 0 の追加描画コストあり |
| `DetailsLazyMeta` メモリ hit | `app.rs:54491,5541` の元属性比較 | 固定世代は実行中不変、表は再起動で破棄。変更不要 |
| 外部渡し PNG | `materializer.rs:274,555-635` の世代 stamp と同一 target | `epub_materializer_stamp_changes_with_generation_not_source_attributes` |
| 外部渡しの結合見開き | `materializer.rs:558-566,937-951` は空 stamp で毎回 miss | 再利用されないことを既存 `lookup_reusable` 判定で確認 |
| 内容同定台帳 | `content_identity.rs:56-77,767-785,1811-1940` | EPUB 種別、固定世代、backfill 来歴、FILETIME の各テスト |
| 保持ラスタ / final AI | `app.rs:65221-65261` は実行中の論理ページ。I7 により世代固定で変更不要 | `pdf_loader::tests::epub_resolver_pins_first_generation_across_retire_and_republish` |
| PDF worker 文書 cache | `pdf_loader.rs:4335-4354` は実ファイルパス。世代ごとに物理パスが異なり変更不要 | 同じ固定世代テスト |
| 類似画像索引 | `similar_index.rs:5698-5715` は `.pdf` のみで EPUB を除外 | 対象外 (ページ索引投入は別段階) |
| Remote ページ数・`PdfIdentity` | `remote_ipc/container.rs:6126-6193` | S4。コードは変更しない |

`pdf_meta` の先出しは実行中の固定表をメモリで読むだけとし、未固定の EPUB と D10 の綴じ方向待ちは列挙結果を待つ。列挙後の EPUB `pdf_meta` 保存は短命 worker が cold catalog の open を含めて行う。`epub_open.begin` と `epub_open.first_display` (`placeholder` 属性) で先出し有無別の初回表示時間を比較できる。Remote のページ数・`PdfIdentity` は S4 の対象で変更しない。類似画像索引は現段階で EPUB を投入しないため、元 EPUB の属性で変換ページを再利用することはない。

内容同定の `ContentKind::Epub` は論理 EPUB パスに付ける。固定済みは不変の世代表から元ファイルのサイズ、完全精度の FILETIME、SHA-256、先頭ハッシュを読み、未固定は元 EPUB を読む。FILETIME から台帳の Unix nanoseconds への変換は既存の範囲制限を明示的に適用し、未固定 EPUB は範囲外時刻の衝突を避けるため毎回 hash する。非同期 backfill は分岐時の固定 ID または未固定の元状態を保存し、書込直前に再確認する。`content_identity.db` はリリース済みなのでスキーマを変えず、EPUB 行の `kind` には旧版が読める `pdf` を保存して、論理 `.epub` キーの読み出し時に `Epub` へ戻す。サムネイル catalog、`pdf_meta`、編集プレビューのリリース済み列にもスキーマ変更はない。EPUB 専用の `epub_cache.db` だけを拡張した。

キャッシュ管理画面は EPUB の現在世代と削除予約を別 worker で取得・更新する。選択・全件削除は I8 の削除予約を追加し、実行中のファイルと固定世代を保つ。RAR/7z/LZH の容量計算と上限には接続しない。

回帰テストは `epub_cache::tests` の世代 ID・再起動・極端時刻・`Stale`・先勝ち・削除予約、`content_identity::tests` の固定世代値と backfill 来歴、`app::tests` の UI 要求・先出し・`pdf_meta` 保存・auto-aspect、`thumb_loader::tests` の要求と catch-up/隣接/代表共通 stamp、`app::metadata_ops::tests` の詳細ページ数、`materializer::tests` の外部渡し stamp、`folder_thumb_pins::tests` の source ID、`edit_preview_cache::tests` の保存・再利用、`archive_cache::tests` の容量除外に追加した。実 PDFium 描画を必要とするページ画像自体の再変換後確認と Remote は、それぞれ利用者の実機確認・S4 に残す。

負例は一時的なコード差し替え後に絞り込みテストを実行して確認し、各実行の `finally` で元のファイルを byte 単位で復元した。元ファイル属性へのフォールバック、世代 ID 固定、元時刻の飽和、来歴チェック除去、編集プレビューの元属性保存、削除予約漏れ、容量への EPUB 加算、固定表の無視、古いページ数の保存、auto-aspect の早期 hit、バッチの行存在判定、ピン ID の元属性使用、詳細・外部渡しの元属性使用は、それぞれ対応するテストを失敗させた。

#### S2c-2 独立レビュー fix round 1

台帳への EPUB 書込は本ごとの固定ロックと同じロックで、来歴の最終確認から SQLite 更新までを一操作にした。通常の固定解決は最初の元ファイル stat / DB 照合をロック外で済ませ、固定直前に本ロック内で元状態を再照合する。固定表全体の mutex はメモリ検索・挿入だけに使い、他の本は別ロックで進める。フォルダ rename / purge / restore copy のように複数の台帳キーを更新する汎用 migration は、対象パス範囲の lease を取る。範囲外の本の固定は待たない。台帳書込経路の再調査結果は次の通り。

| EPUB 論理キーを書ける経路 | 最終確認と書込の所有者 |
| --- | --- |
| 編集 / 閲覧記録、非同期 backfill (`content_identity.rs:1811-2090`) | 分岐時来歴を本ロック内で再確認して `mark_restorable` / `upsert` |
| stage-0 copy detection cache (`content_identity.rs:1590-1700`) | 未固定元状態を hash 前に記録し、本ロック内で再確認して `upsert`。固定済みは元ファイル hash を台帳へ書かない |
| 復元先の台帳昇格と復元拒否 (`content_identity/restore.rs:205-410`) | EPUB 復元先は未固定元状態を本ロック内で再確認。固定済みを昇格しない。拒否行も本ロック内で保存 |
| 空になった編集 origin の clear / 差し戻し (`content_identity.rs:548-595`) | 対象 EPUB キーの本ロック内で flag を更新 |
| 共通 STORES の rename / purge / restore copy (`rename_key_migration.rs:1490-1770`) | `edit_origin.file_key` の対象 exact / 子孫パスを範囲 lease で保護して transaction を実行 |

ドライブ一覧から子フォルダを指すピンの当初の旧行除去は、cache-only 要求で正しい新世代行まで空欄にしたため、次の round 2 で修正した。EPUB ページを指すピンの UI 要求作成では stat を呼ばず、worker 解決印だけを作る。通常 PDF ピンの stat は維持する。キャッシュ管理画面を閉じた際は、アーカイブと EPUB の全削除確認状態を共に消す。

#### S2c-2 独立レビュー fix round 2

- ドライブ一覧の seed は EPUB でも維持する。UI は既読の親代表選定証明の EPUB パス、または型付き source ID の拡張子が EPUB である子ピン ID を要求へ渡す。cache-only worker が子ピン DB を辿り、ID が現行ピン経路と一致した行だけを採用し、固定世代 stamp も照合する。新世代行なら表紙を表示し、旧世代行や子ピン変更後の行は従来の固定ドライブアイコンへ戻す (`app.rs:30259-30495,79852-79911`、`thumb_loader.rs:1066-1177`)。画像・通常 PDF は従来の worker lookup を維持し、`cover.epub.png` は画像である。
- 復元候補は検出時の未固定 EPUB 元状態を保持する。復元 worker は本単位の固定ガード内で来歴・元台帳・復元先状態をコピー前に検査し、各 edit store のコピーと台帳昇格まで保持する (`content_identity.rs:298-306,1729-1738`、`content_identity/restore.rs:147-250`)。この経路の edit_origin コピーは台帳昇格と重複するため省き、ガードの再入も避ける (`rename_key_migration.rs:1176`)。
- 存在しない EPUB ページへのピンは UI で stat せず要求を作り、worker の世代解決失敗時に元のフォルダ自動代表要求を実行する (`app.rs:80915-80951`、`thumb_loader.rs:1215`)。普通の PDF ピンは従来どおり UI で存在確認する。
- `edit_origin` の PDF・画像 exact key の移行とコピーは EPUB 範囲ガードを取らない。EPUB キーが入り得るフォルダ範囲は引き続き保護する (`rename_key_migration.rs:176`)。
- EPUB 仮想フォルダの親子 seed/writeback は除外したまま、PDFium の 100 ms 先読み抑制だけ適用する (`app.rs:27713-27765`)。PDF 側の設定時点は保持する。キャッシュ管理画面の閉じるボタン経路で EPUB 全削除確認を消す (`ui_dialogs/archive_cache_manager.rs:45-80`)。

実経路テストは `drive_list_indirect_epub_cover_matches_current_generation_after_reconversion` (旧世代のみ・現世代行あり・子の明示ピン・子ピン変更後)、`restore_candidates_rechecks_epub_before_copying_edits` (内容同定で候補作成後、置換と固定を行って `restore_candidates_at`)、`missing_epub_page_pin_falls_back_to_folder_representative_in_worker` (`apply_folder_thumb_pin` → `process_load_request`)、`pdf_and_image_store_copy_does_not_wait_for_epub_range` (`run_at` と `copy_stores_at`)、`drive_list_folder_pin_seeds_image_named_cover_epub_png`、`epub_virtual_folder_seed_and_parent_writeback_are_explicitly_excluded`、`window_close_branch_clears_epub_delete_all_confirmation` (egui のタイトルバー閉じる操作) を追加・拡張した。7 箇所を同時に旧動作相当へ一時差し戻したビルドで各テストが失敗し、さらに現行子ピン ID 照合を単独で除くと子ピン変更後の検査が失敗した。差し戻しは `finally` で byte 単位に復元した。

再起動を模した旧世代 2 ページ→新世代 5 ページの fixture で、実際の `process_load_request`、`process_meta_only_with` (列挙だけ fake)、`load_details_page_count_with_pdf_enumerator` (cache hit、PDFium を呼べば失敗)、`MaterializeSession::materialize` (再利用 hit) を通した。4 つの worker 本体を元 EPUB stat に差し戻し、stamp helper は残した負例では各テストが失敗する。固定ロック、stage-0 来歴、復元先の固定判定、範囲 lease、間接 seed、UI pin stat、仮想フォルダ除外、確認状態も個別の旧動作へ差し戻した負例で各テストが失敗した。

#### S2c-2 独立レビュー fix round 3

子 Folder の代表行は UI が catalog の「最新」キーを読み、その source ID を文字列分解する方式を廃止した。実際の書込は `apply_folder_thumb_pin` が選んだ `Seeded` / `AutoSelected` の base key、pin 連鎖の source ID、EPUB 世代 suffix を使う。ドライブ一覧の cache-only worker も `pinned_folder_row_key` で同じキーを組み立て、**現行の**子ピンを設定値そのままの深さで解決して、親 catalog のその一行を完全一致で読む。UI は子 Folder 行を seed しない。行が無い、stamp が違う、WebP が壊れている場合は従来のドライブアイコンへ戻る。非 EPUB の画像・PDF 子ピンも同じ書込キーを読むため代表画像の選択と cache-only 性質は同じだが、既存 WebP の lookup は UI seed から worker の read-only catalog lookup へ移る。直接の画像・PDF ピンは従来の seed 経路を維持する。

`edit_origin` の範囲 lease は `path.is_dir()` を使わない。purge は exact と子孫 prefix の SQL 操作なので、削除後の名前に拡張子があっても prefix として保護する。rename は永続ジョブの `tree`、copy は `Exact` / `VirtualPrefix` の型で区別し、exact は EPUB の拡張子のときだけ取得する。これにより削除済み `books.v2/` の purge と、来歴確認後に停止した EPUB backfill 書込が直列化される。PDF・画像の exact rename/copy は EPUB lease を待たない。

回帰テストは、実書込要求で生成した `|generation:...` キーを親 catalog に置き、`process_load_request` まで通す。旧世代のみ→アイコン、新世代行→表示、子ピンを画像へ変更→アイコン、同一世代のページ 1→0→ページ 0 表示、深さ 1 の子→孫→EPUB 表示を確認する。`drive_list_plain_pdf_child_pin_reads_the_writer_key` は通常 PDF の書込要求が作る suffix 無しの同一キーを worker が読んで WebP を表示する。`deleted_dotted_folder_purge_waits_for_checked_epub_backfill_write` は `write_if_provenance_valid` の確認後・書込前に停止し、実 `purge_removed_paths_at` の完了順と最終台帳を確認する。

負例ではソースの対象関数だけを一時変更し、各 `cargo test --lib <filter>` 後に元の bytes を復元した。「最新行を先に選ぶ・stamp 照合を後回しにする」旧方式で旧世代、画像への pin 変更、同一世代のページ変更の 3 件が個別に失敗した。世代 suffix をキーから省くと新世代行の表示が失敗し、cascade 深さを `depth - 1` に戻すと深さ 1 の経路が失敗した。範囲判定を削除後の `path.is_dir()` に戻すと `books.v2` purge の順序検査が失敗した。さらに `app.rs` / `thumb_loader.rs` / `rename_key_migration.rs` の実装本体を `ee8c38ff9` に一時差し替え、追加テストの compile に必要なキー helper と stamper の `pub(crate)` 露出だけ補って、上記 6 テストを個別に実行した。**6 件とも assertion で失敗**し、`finally` で元の bytes に復元した。compile / harness の失敗ではない。

#### S2c-2 独立レビュー fix round 4

子 Folder の初回 seed は `4a549166c` と同じ親 catalog 読取 (`AutoSelected` base 行、次に `#pin:` prefix の最新行) を同じ UI 経路へ戻した。追加の UI I/O と key 文字列解析は行わない。cache-only worker は seed を暫定表示として送った後、現行子ピン・設定深さ・EPUB 固定世代から書込側と同じ完全一致 key を作る。行が異なれば最終画像で置換し、無ければアイコンへ戻す。同じ stamp・寸法・WebP なら完了通知だけを送り、テクスチャを再アップロードしない (`app.rs` の `drive_list_pin_seed_entry` / `poll_thumbnails`、`thumb_loader.rs` の `send_pinned_only_cached` / `send_pinned_child_folder_cached`)。画像・通常 PDF の初回表示は stage 前の seed と同じ。受け入れた表示上の例外として、再起動後に EPUB が再変換され、子の旧代表が seed されたときだけ、worker 結果が届くまで旧表紙が一時表示され得る。**最終表示**は現行世代行またはアイコンである。

`drive_list_folder_pin_seeds_image_named_cover_epub_png` と `drive_list_plain_pdf_child_pin_reads_the_writer_key` は親 catalog から worker 起動前の seed を確認し、前者は実 `process_load_request` と `poll_thumbnails` を通して同一行でテクスチャ ID が変わらないことも確認する。`drive_list_indirect_epub_cover_matches_current_generation_after_reconversion` は実書込 key の旧 WebP を seed した後、実 worker の現行世代 WebP が UI テクスチャを差し替えることを確認する。`drive_list_child_miss_discards_seed_waiting_for_texture_budget` は seed が upload 待ちの間に worker が miss を返す順序を検査し、古い seed を待ち行列から除く。

子フォルダの pin 先が移動・削除されて解決できない場合、cache-only worker は書込側と同じ `AutoSelected` base key を読み、保存済み自動代表があれば表示する。行が無ければ固定ドライブアイコンへ戻る (`thumb_loader.rs` の `send_pinned_child_folder_cached`)。`drive_list_missing_child_image_pin_uses_cached_auto_representative` は実際の drive-list request と `process_load_request` を通す。旧「解決不能なら即終了」に一時差し戻すと画像なしで失敗した。

削除要求は UI が削除前の `GridItem` から決めた `DeleteSourceScope::Exact / Tree` を worker の Shell 成功後まで運ぶ。`edit_origin` の範囲ガードは削除後のファイル有無に依存せず、実際の SQL 範囲に従う（round 5 の詳細を参照）。`delete_worker_guards_exact_descendants_without_waiting_for_unrelated_keys` は実 worker tail と実 purge を通し、重なる EPUB 子孫を持つ Exact と削除済み dotted フォルダの Tree は待ち、重ならない Exact は待たないことを検査する。失敗後の公開済み削除 journal は scope を保存しない。復元した scope は不明なので `Tree` とみなし、EPUB 範囲ガードを保守的に取る。非 EPUB ファイルで余分な待ちがあり得るが、正しさを失わない。`restored_delete_purge_journal_conservatively_covers_epub_descendants` は実 journal 書込・再読込・retry でこれを検査する。journal 形式は変更しない。

負例では変更箇所を一時的に旧分岐へ戻し、各テストの assertion 失敗を確認して元の bytes を復元した。子 Folder seed を `3b002372a` の `None` に戻すと画像・通常 PDF の両テストが失敗し、worker の完全一致 lookup を無効にすると EPUB の新世代差し替えテストが失敗した。同一行判定を無効にするとテクスチャ ID の検査が失敗し、miss 時の待ち行列除去を無効にすると旧 seed が残る検査が失敗した。journal の復元 scope を `Exact` に変えるとガード待機の検査が失敗した。pin 解決不能時の自動代表 fallback と in-process `Exact / Tree` の負例も、それぞれ旧早期 return、`Exact→Prefix` / `Tree→Exact` に戻して失敗を確認した。

#### S2c-2 独立レビュー fix round 5

`90dfc7f7b` では `Exact` の非 EPUB ファイルを EPUB 範囲ガードから外していたが、`purge_store` と `migrate_store` の SQL は対象 key 自身に加えて `<key>/…` と `<key>::…` を処理する。この 3 集合をガード側でも同じように扱う。`<key>/…` は物理フォルダ配下の画像・本などの path-key 行、`<key>::…` は ZIP/CBZ・直接閲覧 RAR/CBR・変換アーカイブのエントリ、および PDF/EPUB の `page_N` のページ別編集・★・タグ等に使われる。ネスト ZIP の entry 名には `::` の後にも `/` が入る。`copy_stores_at` の Exact は key のみ、VirtualPrefix は `<key>::…` のみなので、それぞれ SQL と同じ範囲を指定する。公開済みストアの schema と削除 journal 形式は変更しない。

範囲ガードは `path_key::normalize_keep_drive` と同じ規則で小文字化し、`\\` を `/` に揃え、ドライブ文字を保持する。範囲は「完全一致 key」「`key/` prefix」「`key::` prefix」の集合として保持する。2 範囲は、それぞれの完全一致 key が相手の prefix に入るか、prefix 同士が包含関係にある場合だけ重なる。EPUB 本の pin・台帳書込は自分の正規化 key を含む範囲だけ待つ。重ならない範囲は並行し、共有状態の mutex は登録・解除の短時間だけ保持する。待機は `Condvar` で行い、UI スレッドの待機とロック順の逆転は追加しない (`pdf_loader.rs`、`rename_key_migration.rs`)。round 4 の「Exact は非 EPUB ならガード不要」という記述はこの段落で置き換える。journal 復元時の scope 不明を Tree とみなす規則は維持する。

`exact_plain_file_purge_cannot_delete_later_epub_ledger_write` は、`cover.png` の Exact purge がガードを取って SQL を実行する直前に停止し、`cover.png/book.epub` の台帳書込が purge 後まで待って行を残すことを検査する。`exact_purge_keeps_slash_and_virtual_descendant_deletion` は Exact でも `/`・`::` 子孫行を従来どおり削除する。`delete_worker_guards_exact_descendants_without_waiting_for_unrelated_keys` は Shell 成功後の worker purge で重なる Exact / Tree は待ち、無関係な Exact は待たないことを確認する。`nonoverlapping_epub_ledger_ranges_run_concurrently` と `pdf_and_image_store_copy_does_not_wait_for_nonoverlapping_epub_range` は別範囲の同時進行を、`overlapping_epub_ledger_ranges_wait_for_each_other` は重なる範囲の待機を検査する。`epub_ledger_key_ranges_normalize_case_and_separators` は大文字・逆スラッシュと隣接名の境界を確認する。

ドライブ一覧の子ピンが解決不能になった際の自動代表も、通常の子フォルダ読取と同じ `folder_cached_row_usable` を通してから送信する。EPUB 世代だけでなく選定元ファイル、pin revision、依存 catalog の選定証明を検証する。`drive_list_missing_child_image_pin_uses_cached_auto_representative` は実際のフォルダ代表 writer が選定証明付きで保存した行を drive-list worker から読む。有効時は表示し、選定元画像の削除後はアイコンへ戻す。

負例は対象分岐だけを一時的に変更し、各テストが compile ではなく assertion で失敗した後に元の bytes を復元した。`Exact` を非 EPUB 拡張子ではガードから除く `90dfc7f7b` の規則に戻すと `cover.png/book.epub` の後続台帳行が purge に消される。自動代表の共通証明検証を外すと削除済み画像の WebP が表示される。範囲を全件直列に戻すと非重複の同時進行テストが、重なり判定を無効化すると重複範囲の待機テストが失敗する。SQL の子孫 prefix を狭めると Exact の `/`・`::` 削除テストが失敗する。

#### S2c-2 独立レビュー fix round 6

`dd589a59d` では範囲ガードの取得が purge 内であり、Shell 成功から purge 開始までに新しい EPUB の台帳行を書けた。削除 worker は要求時に型付きで保存した `Exact / Tree` の全範囲について、最初の Shell 処理より前に RAII 範囲ガードを 1 回取得する。キャンセル、Shell の一部失敗、複数チャンク、purge の再試行を含め、成功した範囲の最後の purge と失敗 journal 記録まで保持する。purge は渡された同じガードを使用し、内側で再取得しない。SQL の exact・`/`・`::` 範囲と round 5 の重なり判定は維持する (`delete_worker.rs`、`rename_key_migration.rs`、`pdf_loader.rs`)。

保持中に待つのは、同じ範囲内の EPUB 本の pin と台帳 backfill 書込、および重なる別の範囲操作である。pin と backfill は worker 上で走り、UI スレッドはガードを取得せず結果を非同期に受ける。Shell の確認・進捗 UI が開いても UI スレッドはこのガードを待たず、範囲ガードの mutex は登録・解除時だけ保持するため、確認操作との待ち合わせ循環はない。retry journal は公開済み形式を変えず、読み出した全 path を scope 不明の `Tree` として、孤立判定と purge より前に同じ RAII ガードを取得する。journal lock はガード取得前に解放し、更新時だけ取り直す。

`shell_delete_keeps_epub_backfill_out_until_purge_finishes` は実 delete worker の fake Shell で `cover.png` を消した直後に停止し、同名ディレクトリ内に `book.epub` を作り、実 `run_backfill_at` から書込を開始する。Shell の停止中は書込が完了せず、worker が実 purge を終えた後に新しい台帳行が残ることを検査する。ガード取得を `dd589a59d` と同じ Shell 後・purge 前へ一時的に戻すと assertion で失敗し、元の bytes に復元した。

EPUB 以外の台帳・編集行には今回の per-book ガードがない。例えば `cover.png/x.jpg` の編集行について Shell と purge の間に行が作られる窓は master にもある既存の制約であり、この段階では変更しない。

### S2c-2 検収 (2026-09-26)

独立レビュー 7 回目で承認 (`9359baab9`)。1〜6 回目の指摘は修正 1〜6 で対応した。検収時の全ライブラリテスト 9,264 件、
変換器 35 件が成功。受容した制約: (1) 再起動後に再変換された EPUB が子フォルダーの代表のとき、ドライブ一覧で worker の
結果が届くまで旧表紙が一時表示され得る (最終表示は常に現行世代)。(2) 削除の確認ダイアログ表示中は、その範囲の EPUB の
固定・backfill・移行が待つ (UI スレッドは待たない)。(3) Shell 削除から purge までの窓は非 EPUB 行では master から既存で範囲外。
別課題へ切り出したもの: 並列テストで `RECORD_SEQUENCE` を共有する既存の不安定テスト。残る P3: `shell_delete_keeps_epub_backfill_out_until_purge_finishes`
の開始通知は高負荷時に退行を見逃し得る (テストの限界)。実機確認 (再変換後の表示・キャッシュ管理画面・先出しの計測) は未実施。

### S3a 検収 (2026-09-26)

独立レビュー 3 回目で承認 (`3277fde39`)。1〜2 回目の指摘は修正 1〜2 で対応した。設計判断: 検索索引の `INDEX_VERSION` は
据え置き (旧版へ戻した場合の影響は上記のとおり)、EPUB はファイル名・パスでのみ検索対象 (書誌情報の索引は後の課題)。
全ライブラリテスト 9,278 件成功 (レビュー担当が再実行)。実機確認 (一覧表示・D5・★固定・外部アプリ設定) は未実施。

### S3b 実装記録 (2026-09-26)

- D12 の保存は同じフォルダの `.pdf` へ行う。データ領域の作業フォルダで既存の PDF ページ数検証を通し、保存先フォルダの記録済み一時ファイルへ複写した後、同じボリューム内で上書きしない公開操作を行う。失敗・取消時は一時ファイルと作業フォルダを削除する。
- 元 EPUB の現在状態と出力版が一致する現行世代があれば PDF を複写して再検証する。なければ通常の worker と profile で作業フォルダへ変換し、`epub_cache` の世代は作成しない。PDF Info の `/Title` と `/Author` は EPUB の `dc:title` と最初の `dc:creator` から作る。既存の `/ViewerPreferences /Direction` は維持する。
- 公開後のデータ引き継ぎには通常のファイルコピー用 `copy_restore_stores_without_identity_at` を exact と仮想ページ prefix の両方へ適用する。レーティング・ページ補正などは EPUB と PDF の両方に残る。内容同定の行は PDF へ複写しない。しおりとコレクション登録も通常のコピーと同じく複写せず、元 EPUB に残る。
- 確認画面では同名 PDF の有無を inspect worker が調べ、存在する場合は保存ボタンを理由付きで無効にする。保存結果は元の open owner・履歴・フルスクリーン予約を保って通常の PDF として開く。スマートフォルダとコレクションの参照元は EPUB のままにする。バッチは選択 EPUB を順に処理し、同名 PDF・失敗を個別表示し、処理中の 1 件の後で停止する。完了後に一覧を更新して D5 を反映する。
- ZIP/RAR を `ArchiveFormat` によって分類する既存バッチとは EPUB の入力型を共有しないため、`ConvertSource` は導入しない。
- 実経路の回帰テストでは、現行世代の再利用と古い世代の再変換、キャッシュ外パスの除外、PDF 検証後の上書き禁止、取消・書込失敗時の `.part` 除去、通常コピーで扱う設定の複写と内容同定・しおり・コレクションの非複写、ダイアログの所有権と案内、バッチ結果・停止・D5 再走査、PDF Info を確認する。実装条件を一時的に外す感度確認でも対応するテストの失敗を確認した。
- S3b の自動検証: `cargo test -p mimageviewer --lib` は 9,299 成功・47 ignored、`cargo test -p epub-pdf-worker` は 37 成功。core check、fmt、glyph lint、開発用 core・remote・EPUB worker の build-dev も成功。共通 `test-full.ps1` は、この worktree に release 版 core・remote がなく launcher の build script で停止したため、S3b の合否には上記の指定ゲートを用いる。実際の WebView2 による 27 冊変換と画面操作は設計担当の実機確認に残す。
- S3b 実機検証で固定ページの PDF に印刷用 URL を持つ Chromium の古い Info 辞書が残ることが判明した。merge は最終 trailer を新しい書誌 Info へ向けていたが、各印刷パートの Info オブジェクトも出力へ複写していた。merge 時に元 Info を除外し、OPF に書名がないときは EPUB のファイル名 (拡張子を除く) を `/Title` にする。著者がなければ `/Author` は置かない。固定パート複数と書名なし単一パートの実 merge テストで、印刷用 URL が出力の Info に残らないことを確認する。
- S3b 実機再検証 (`target/epub-spike/run8`) で reflowable 3 冊のページ内 Link 注釈に、仮想ホスト `epub.invalid` を指す URI action が残ることが判明した。merge 時に URI の host が仮想ホストと一致する Link 注釈を `/Annots` から外し、参照されなくなった注釈と action 辞書を除去する。実際の外部ホストへのリンクは維持する。**既知の制限**: EPUB 内の目次・章リンクを PDF 内の GoTo 移動へ変換する処理は未実装であり、該当リンクは保存 PDF では使えない。GoTo 化は後続課題。
- S3b 独立レビュー修正 1: `epub_cache.db` の世代行に `output_version` を追加した。旧行・旧スキーマは版 0、現行版は `epub_cache::CONVERTER_OUTPUT_VERSION`。worker の PDF 出力形式 (ページ・書誌情報・リンクなど) を変えるたびに定数を上げる。旧世代は閲覧可能なまま残すが、同名 PDF へ保存する際は現行版だけを再利用し、旧版は再変換する。この DB は未リリースなので列追加の対象は開発中の既存行のみ。
- 保存 worker の出力と worker 自身の `.tmp-*` はデータ領域の `epub_sibling_work/epub-<PID>-<番号>/` へ置く。そこで検証後、保存先には `.miv-part-<乱数>.tmp` を排他的に作って複写し、上書きなしで公開する。固定長の名前で長い EPUB ファイル名でもファイル名長の上限を超えない。作成前に `epub_cache.db` の `outstanding_sibling_outputs` へ保存先と temp パスを予約し、作成後にファイル識別子を記録する。死亡した PID の作業フォルダは既存の安全な一時フォルダ削除手順で掃除する。
- host は元 EPUB のファイル名から拡張子を除いた値を worker の `--source-stem` へ渡す。作業コピー `source.epub` に書名がなくても、PDF `/Title` の代替値は元の名前になる。EPUB のバッチ変換画面も共通モーダル入力ブロックへ登録し、一覧のキー操作を止める。
- S3b 独立レビュー修正 2: 保存先一時ファイルの名前には UUID v4 を 2 個連結した 244 bit の乱数を入れる。DB 行は `reserved` と `created` の 2 段階。`create_new` 失敗時は予約だけ消し、既存ファイルには触れない。作成成功時の開いたハンドルから NTFS のボリューム番号とファイル ID を取得して `created` 行に記録する。回収時は再解析ポイントを拒み、開いた同一ハンドルで識別子を照合し、そのハンドルからだけ削除する。`reserved` 行および新しい列を持たない開発中の旧行は削除対象とせず行だけ閉じる。**許容する残置**: 作成成功から `created` 記録までの間に異常終了すると、乱数名のファイルが残る。この窓では所有証明が永続化していないため自動削除しない。
- 保存先の残置回収は起動ゲート成立後の背景スレッドで行い、1 件ごとに失敗を記録して行を残す。アクセス不能な保存先が EPUB 自体の起動を止めない。作成済み行の親フォルダが見つからない場合も、共有先の一時的な不達を考慮して行を残す。公開成功後の行削除が失敗しても、PDF 保存・データ引き継ぎ・表示は成功として続ける。残った行は、親フォルダに到達でき一時ファイル不在を確認した次回起動時に閉じる。単独の保存ダイアログは `Saving` でも共通モーダル入力遮断の対象であることをキー操作テストで確認する。
- 修正 2 の検証: 既存同名一時ファイルの誤削除テストは `fd3ba8865` の処理で失敗し、作成済み行の親フォルダ不達テストも修正前に行が失われて失敗した。ファイル識別子照合と公開後エラーの非致命扱いを一時的に外すと、対応する 2 テストが失敗し、復元後は両方成功した。修正後の全 lib は 9,315 成功・47 ignored、EPUB worker は 43 件成功。core check、fmt、glyph lint と `build-dev.ps1` による core・Remote・EPUB worker の再構築が成功した。Clippy で変更箇所に新しい警告はない。感度確認後の `sibling_` 再実行で既存の複数窓非同期待機テストが 1 回タイムアウトしたが、同テストの単独再実行は成功した。実 WebView2 での 27 冊変換と画面操作は設計担当の再確認待ち。
- S3b 独立レビュー修正 3: 128 bit のファイル ID が全ゼロ、または ID 取得が失敗した場合は所有を証明できないため、`unprovable` 行として記録し、自動削除しない。起動後の背景回収はその行だけを閉じ、乱数名の残置ファイルは許容する。64 bit の旧ファイルインデックスへはフォールバックしない。保存先の一時名は `.miv-part-<64 桁の乱数>.tmp` とし、作成時の Hidden 属性は best effort、通常一覧と一括名前索引では属性に関わらず名前で除外する。開発途中の旧 `.pdf` 一時名も除外・既存の識別子つき回収対象にする。作業 PDF は PDFium 検証の前から読み取り共有だけを許すハンドルで保持し、検証後も同じハンドルから複写する。保存先も読み取り共有だけを許す作成ハンドルを公開まで保持し、作業 PDF の複写時 SHA-256 と保存先ハンドルからの再読込 SHA-256 を照合する。Hidden を外してから同じハンドルの `FileRenameInfo` (置換なし) で公開する。不一致・公開失敗では同じハンドルから削除する。
- 修正 3 の検証: `sibling_zero_file_id_is_unprovable_and_gate_never_deletes` はゼロ ID 判定を外すと失敗した。一時名の除外を外すと `sibling_output_temp_names_are_internal_even_without_hidden_attribute`・実フォルダ走査・一括名前索引の 3 テストが失敗した。保存先共有制限、再読込ハッシュ比較、置換禁止、検証前の作業 PDF 共有制限を個別に外すと、それぞれ `sibling_temp_denies_concurrent_write_until_handle_publish`、`sibling_save_readback_hash_mismatch_removes_temp_without_publishing`、`sibling_handle_publish_never_replaces_pdf_created_after_temp`、`sibling_save_holds_verified_work_file_write_denied_through_publish` が失敗した。復元後、保存関連 67 テストと全 lib 9,323 成功・47 ignored、EPUB worker 43 成功。core check、fmt、glyph lint、`build-dev.ps1` による core・Remote・EPUB worker 配置と PE 依存検査も成功。ビルドの既定待機は別 worktree の MSBuild ノードが残ったため中断し、既存手順の `-WaitForOtherBuildsMinutes 0` で完了した。実 WebView2 の 27 冊再実行と GUI 操作は設計担当の確認待ち。
- 修正 1 の検証: `cargo test -p mimageviewer --lib` は 9,308 成功・47 ignored、`cargo test -p epub-pdf-worker` は 43 成功。core check、fmt、glyph lint、`build-dev.ps1` による core・Remote・EPUB worker と PE 依存検査も成功した。別 worktree の MSBuild 待機ノードが残ったため、ビルドはスクリプトの `-WaitForOtherBuildsMinutes 0` を指定して実行した。保存先 worker 出力の旧経路は取消テストで失敗し、出力版・起動時回収・モーダルの判定を一時的に外すと対応する 6 テストが失敗した。書名代替値を旧処理へ戻すと `/Title=source` で失敗した。実 WebView2 の 27 冊再実行は設計担当の確認待ち。
- S5 で反映済み: 明示保存した PDF と EPUB 内の書誌情報について、`privacy.html` の日本語・英語を更新した。S3b では編集しなかった。

### S3b 検収 (2026-09-27)

独立レビュー 4 回目で承認 (`4345f8597`)。1〜3 回目の指摘は修正 1〜3 で対応した。設計判断: 利用者データのコピー範囲は
mIV の通常のファイルコピーと同じ (しおり・コレクション登録・内容同定は EPUB に残る)。保存は現行出力版の世代だけを再利用し、
保存先の一時ファイルは乱数名 `.miv-part-<token>.tmp`・ファイル同一性で所有を証明できたものだけを起動後に回収
(証明できないものは残す)。検証済みの内容を同じハンドルで照合して置換なしで公開する。設計担当の実 WebView2 27 冊変換 (run10):
状態・ページ数不変、全出力に OPF の書名、`epub.invalid` 0、外部リンク 80 維持。検収時の全ライブラリテスト 9,323 件・変換器
テスト成功。残る課題: EPUB 内リンクの PDF 内 GoTo 化、EPUB の書誌情報の検索索引。実機確認は未実施。

### S4 mIV Remote (2026-09-27)

- 本体の通常フォルダー一覧は S3a の共通 materializer と live `skip_epub_if_pdf_exists` を使う。EPUB タイルは既存の `RemoteEntryKind::Pdf`、開いた本は `ContainerKind::Pdf`。本・ページの address と保存キーには元 `.epub` を保持し、ページ描画だけ `pdf_loader` が固定世代へ解決する。アーカイブ先行分岐は EPUB を ZIP と判定せず、アーカイブ変換ジョブも EPUB を受け付けない。
- Remote のページ数メモリキーと親 catalog の `pdf_meta` は EPUB の世代 ID / 世代 PDF サイズで照合する。開く操作は一度固定した `ReadTarget` をページ数・D10・見開き寸法まで通し、見開き用 catalog の個別再取得でも同じ stamp の行だけを採用する。ページ・表紙要求も範囲確認前に固定して要求と singleflight キーを同じ stamp へ差し替え、thumbnail worker へ target を渡す。PDF worker プロセスへはその不変世代の物理パスを渡す。通常 PDF のファイル時刻・サイズとパスワードは維持する。未変換の本を開いたときは PC での変換を案内し、起動時に EPUB が使えない場合は別の案内を返す。Remote から変換は始めない。
- Web は既存の `pdf` wire kind を使って EPUB を PDF 本として遷移させ、タイルには EPUB バッジを出す。変換済みの表紙は本体のページ 0 を読み、未変換は EPUB プレースホルダーにする。wire のフィールドと enum 値は同じだが、`pdf` kind が EPUB 論理パスも含むようになったため protocol v61 へ上げ、本体と remote-web の版を揃える。
- Remote の見開き・綴じ方向は PC と同じ `spread.db` の本キー `x.epub` と共通のページ組みを使う。D10 が ON で本別モード・方向が未保存の場合、EPUB は固定世代の方向、通常 PDF は `/ViewerPreferences /Direction` を既定値へ適用する。PDF の方向列挙は ON の場合だけ行い、OFF では追加処理をしない。保存済みの方向だけがある場合も PC と同様に既定モードを回転する。新しい Remote 設定や IPC 変更はない。
- S4 修正 2: 見開き用 catalog の世代一致行でも寸法列が空なら、開く操作で固定済みの `ReadTarget` からページ寸法を一括取得する。個別再取得中に別世代の縦横比へ書き換わっても、固定世代の横長判定を維持する。通常 PDF は空寸法行だけでは従来どおりページ寸法を追加取得しない。D10 OFF の通常 PDF はページ数キャッシュが無く列挙が必要な場合も `want_direction=false` とする。
- 修正 2 の検証: 旧世代が横長・新世代の catalog 行が縦長の競合テストは、修正前の空寸法判定に戻すと Remote の分割表示が 2 グループから 1 グループになり失敗した。D10 OFF・ページ数キャッシュなしのテストは、列挙時の `want_direction` を一時的に `true` にすると失敗した。復元後、全 lib 9,335 成功・47 ignored、Remote IPC 59 成功、Remote Web 123 成功・1 ignored、Web JS 412 成功。core check、fmt、glyph lint、`build-dev.ps1` による core・Remote・EPUB worker の配置と PE 依存検査が成功した。

### S4 検収 (2026-09-27)

独立レビュー 3 回目で承認 (`03eb042a6`)。1〜2 回目の指摘は修正 1〜2 で対応した。設計判断: D10 は Remote でも PDF・EPUB の
両方に効く (設定 ON かつ本別の保存値なしのときだけ方向を取得)。IPC は wire 変更なしだが `pdf` 種別の意味が EPUB へ広がったため v61。
検収時: 全ライブラリ 9,334 件成功・1 件は既存の類似索引テスト (5 秒の待ち上限) が並列負荷で時間切れ、単独 3 回とも成功 (S4 は当該
ファイル未変更)。remote-ipc 59 件、remote-web 123 件、Web JS 412 件成功。Remote の実機確認は未実施。

### S5 配布・文書 (2026-09-27、実装・静的検証完了、配布ビルドと独立レビュー待ち)

- launcher は EPUB converter を core・remote と同じ版別 runtime へ展開し、SHA-256 で照合する。署名する配布ビルドでは内包前に converter を署名する。core の既存探索は自分の exe の隣の `mimageviewer-epub-pdf.exe` を指す。portable は `target-portable` で worker をビルドし、core exe の隣へ複製する。
- `build-release.ps1` は core → remote → EPUB worker → 内側 3 exe の署名 → launcher → launcher の署名の順とした。`build-release.sh` も同じビルド順。`build-dist.ps1` の clean・稼働プロセス確認・最終 PE 検査、portable の稼働プロセス確認・存在確認・署名・診断ビルド manifest に worker を含める。
- `installer/mimageviewer.iss` の配布ファイル一覧は launcher だけで正しいため、埋め込み対象のコメントのみ更新した。readme、マニュアル、製品ページ、プライバシー文書の日本語・英語、構成・配布文書を更新した。Remote マニュアルは S4 の文言が変換済みのみ閲覧できる仕様と一致するため変更不要。`docs/architecture-overview.md` と `docs/async-architecture.md` には S2 の起動ゲート、Job Object、世代寿命の記述が既にあり、前者は配布配置も追記した。
- 設計担当の判断: 一時データの保存先は実装どおり記述し、変更しない。閲覧用の通常変換は Windows の一時フォルダ、portable 版はアプリのデータフォルダ、同じ場所への PDF 保存は全版でデータフォルダ内の作業領域を使う。管理者・利用者の WebView2 ポリシーが別の保存先を指定した場合は従い、アプリはその別領域を削除しない。アプリ所有の一時フォルダは変換後と次回通常起動時に削除を試みる。
- 公開文言の利用者判断: 製品ページ「安心して使えます」は元の見出し・説明文を維持し、EPUB の通信と明示保存を短く追記した。EPUB 以外の画像送信・Remote・外部ツールの既存文言は 38006d8cd に戻した。プライバシー文書の日本語・英語には、EPUB 内の外部サイト要求の遮断、一時データ、ポリシー、キャッシュ PDF、明示保存時だけの元 EPUB の隣 (ネットワークフォルダを含む) への出力を追記した。保存 PDF は EPUB の書名と、存在すれば最初の著者をメタデータに記録し、書名が無ければ元ファイル名を使う。キャッシュ削除は同じデータフォルダを使う全アプリ終了後の起動時と明記した。
- 初回の自動検査: `test-build-dev-safety.ps1`、`test-release-build-safety.ps1`、fmt、glyph lint、HTML パーサ、PowerShell 変更行の ASCII 確認、`git diff --check` が成功。初回の launcher unit test は worktree に内包元の release exe 3 本がないため build.rs の事前検査で停止。実装担当は配布・release・portable ビルドを実行していない。
- 独立レビュー 1 回目の P2/P3 修正: launcher は EPUB worker と CRT を、版別 runtime の `.sha256` が現行でも毎起動時に実体ハッシュで照合し、core・remote・FFmpeg は従来の sidecar 近道を保つ。同じ長さで壊した worker と現行 sidecar の回帰テストは修正前に失敗、修正後に成功した。製品ページの明示保存文言は右クリック変換も含む短文にし、手順書の段数と前提ファイル数の表現を直した。設計担当が release 入力 exe を作成した後、launcher 11 件、fmt、glyph lint、HTML パーサ、`git diff --check` が成功した。独立再レビュー待ち。

### S5 検収 (2026-09-27)

独立レビュー 2 回目で差分を承認 (`1bf4a52a1`)。利用者決定: EPUB 以外の既存の公開文言は変えない (正確さの見直しは別課題)、
製品ページの EPUB は短く、詳細は privacy.html。設計担当の検証: release の core・remote・worker・launcher を署名なしでビルド、
launcher テスト 11 件成功、portable の core・worker をコンパイル。**未実施**: `build-portable.ps1` / `build-dist.ps1` の通し実行と
署名 (他の mImageViewer が起動中で `-PreserveRuntime` が停止を拒否したため)。リリース前に mImageViewer を閉じて実行し、
配布成果物 (単体 exe・インストーラ・portable zip) に worker が入り署名されていることを確かめる。

### S5 後の開き方設定の分離 (2026-09-27、D13)

RAR の「次回から表示しない」が EPUB にも効いた実機確認を受け、EPUB 専用の 3 択を追加した。既定は確認ありで、旧書庫設定からの移行はしない。通常一覧・フォルダ移動・スマートフォルダ・Remote のフォルダ・コレクション・タグ一覧の EPUB 表示判定と、変換確認ダイアログの省略判定を独立させた。RAR / 7z / LZH は従来の設定を読む。
