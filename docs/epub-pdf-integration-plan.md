# EPUB → PDF 変換の本体組み込み設計 (2026-09-25、第 3 版 = 独立レビュー 2 回目の反映)

- 経緯と検証: `docs/backlog-on-hold.md` §1.247 (計画)、`docs/epub-pdf-spike-results.md` (スパイク結果)。
- 変換器: `crates/epub-pdf-worker` (単体 CLI `mimageviewer-epub-pdf`)。
- 状態: **設計案 第 3 版。独立レビュー 3 回目待ち**。各回の指摘と対応は §8。
- 行番号は `epub-pdf` ブランチ (master `a887ba43a` 起点) のもの。

## 1. 決定事項 (利用者合意 2026-09-25)

| # | 決定 |
| --- | --- |
| D1 | DRM のない EPUB を固定レイアウトの PDF へ変換し、既存の PDF 経路で読む。汎用 EPUB ビューアにはしない |
| D2 | 変換結果は APPDATA の変換キャッシュ (`archive_cache`) に置く (案 A)。元ファイルの隣には書かない。NAS / 読み取り専用メディアでも動くこと |
| D3 | ページ単位・本単位の利用者データ (補正・回転・レーティング・タグ・注釈・消しゴム/モザイク・読書位置・しおり・履歴・コレクション・代表サムネ・見開き設定等) は**元の EPUB のパス**をキーにする。キャッシュの削除・置き場所に影響されない |
| D4 | EPUB 由来の PDF は容量上限による自動削除の対象外 (合計にも数えない)。キャッシュ管理画面の明示操作でのみ消す |
| D5 | 同名の PDF がある場合は EPUB を一覧から隠す設定を追加し、既定 ON (`skip_archive_if_zip_exists` と同型) |
| D6 | リフロー型は電子書籍端末相当の**固定**ページ寸法で組む。設定は作らない。本の CSS は上書きしない |
| D7 | 朗読音声・動画は PDF に入らない。マニュアルに明記する |
| D8 | WebView2 の束縛は `webview2-com` (Loader 静的リンク、DLL 同梱不要) |

## 2. 利用者から見た挙動

1. フォルダに `book.epub` があると、一覧に 1 冊の **PDF 系の本**として出る (バッジ「EPUB」)。未変換ならアイコン、変換済みなら 1 ページ目のサムネイル。
2. 開くと、未変換なら確認ダイアログ → 変換 (進捗・キャンセル可) → 開く。変換済みなら即座に開く。確認の要否は既存の `archive_file_handling` (Ask / Convert / Ignore) に従う。どの入口 (クリック・フォルダ移動・履歴・しおり・起動復元) から開いても同じ。
3. 開いた後のアドレスバー・親へ戻る・次回起動の復元・履歴・しおりは、すべて `book.epub` を指す。PDF の本と同じ種類として扱われる。
4. DRM 付き / 壊れた EPUB / WebView2 Runtime なし / タイムアウトは、それぞれ理由を明示したエラー。空の本にはしない。
5. 本に保存された見開き設定が無ければ、EPUB の `page-progression-direction` に従う (右開きの本は右開きで開く)。
6. キャッシュ管理画面で EPUB の変換結果を消すと「削除予約 (次回起動時に削除)」になる。

## 3. 中心設計

### 3.1 考え方: 論理パスは EPUB、PDFium が読むのは変換世代のファイル

EPUB を「**中身のバイト列が変換キャッシュに置いてある PDF の本**」として扱う。

- アプリ内で本とページを識別するパス (論理パス) は、**どこでも `x.epub`**。一覧のタイルは `GridItem::PdfFile(x.epub)`、
  中身は `GridItem::PdfPage { pdf_path: x.epub, .. }`、`current_folder = x.epub`。PDF の本と同じ種類。
- PDFium にファイルを開かせる処理は、すべて `pdf_loader` の公開関数 → `encode_*_request` → ワーカープールを通る
  (`src/pdf_loader.rs:884-964,3919-4640`)。**論理パス → 変換世代ファイルの解決は、`pdf_loader` の公開関数の入口で、
  要求を組み立てる前に 1 回だけ行う** (§3.3)。
- PDFium 以外で本のファイル自体を扱う処理 (内容同定のハッシュ `content_identity.rs:1786`、タイルの「外部で開く」
  `external_tool.rs:223`、ドラッグ出し `ui_main.rs:1917`、コピー・削除・改名) は、**論理パス = 元 EPUB を扱うのが正しい**。
  これらは変更しない。PDF ページの外部渡しは PNG 化 (`materializer.rs:244-249`) なので描画は `pdf_loader` を通る。

### 3.2 不変条件

- **I1 識別の一意性**: 本・ページの識別・永続キー・表示・履歴に使うパスは `x.epub`。変換世代ファイルのパスは
  `pdf_loader` の要求の中と、キャッシュ管理の外へ出さない (ログを除く)。
- **I2 解決は要求の組み立て前に 1 回**: `x.epub` → 変換世代ファイルの解決は `pdf_loader` の入口だけが行う。
  その後の要求 (open admission の文書 ID、列挙の合流キー、ワーカーの文書キャッシュ) はすべて**実際に読むファイル**で動く。
- **I3 未変換は型付きエラー**: 有効な変換世代が無い / 元 EPUB の mtime・size が記録と違う場合、解決は
  `PdfReadError::NotConverted` を返す。空の本・黙った fallback にしない。
- **I4 派生データは読むファイルのスタンプで検証**: PDF の中身から作った派生データの鮮度判定は、論理パスの stat ではなく、
  **実際に読むファイルの (mtime, size)** を使う (§3.4)。
- **I5 UI スレッドで DB もファイルも引かない**: 解決は UI スレッドで行わない。`pdf_loader` の同期 API は既に UI 外からしか
  呼ばれておらず (レビュー 2 回目で確認)、UI 側で encode している `enumerate_pages_async` は encode と解決をスレッド側へ移す。
- **I6 変換世代ファイルは不変で、プロセス実行中は消さない**: 一度公開した世代ファイルは書き換えない。再変換は新しい世代を作る。
  廃止した世代 (再変換・キャッシュ削除・元ファイル消失) は DB に「削除予約」として記録し、**次回起動時、PDF ワーカーを
  起動する前に**消す。

### 3.3 解決の場所 (第 2 版の読み取りリースを廃止)

第 2 版はディスパッチャで解決し、読み取りリース (参照カウント) で世代の削除を遅らせる案だった。レビュー 2 回目で、
(a) ディスパッチャは取り出す前に論理 `document_identity` で open admission を決めて枠を計上する
(`pdf_loader.rs:3037,3094,3117`)、(b) 投入済み要求の本体も論理パスで符号化済み (`pdf_loader.rs:1915,3322,3367`)、
(c) DB 行の切替とリース登録が原子的でない (`archive_cache.rs:242,373`)、(d) PDFium ワーカーは直前の文書を
開いたまま保持し、個別の close 要求が無い (`pdf_loader.rs:494,517,621`)、と指摘された。

第 3 版は **I6 (実行中は消さない)** によって、リースと原子性の問題そのものを無くす:

- 解決関数 `pdf_loader::resolve_read_target(logical) -> Result<ReadTarget, PdfReadError>` を、各公開関数
  (`get_document_info` / `get_page_sizes` / `analyze_page_content_type` / `enumerate_pages*` / `render_page*` /
  `enumerate_pages_async` のスレッド側) の先頭で呼ぶ。`ReadTarget { read_path, stamp: (mtime, size) }`。
  - 拡張子 pdf → `read_path = logical`、stamp は論理パスの stat (現状と同じ)。
  - 拡張子 epub → 元 EPUB の stat → `archive_cache.db` の有効行 (元の mtime・size 一致、削除予約でない) → 世代ファイルの stat。
    行なし・不一致・削除予約 → `NotConverted`。
- 世代ファイルは実行中に消えないので、解決した `read_path` は要求の完了まで有効 (待機中の要求は旧世代で完了し、
  新規の要求は新世代を読む)。open admission・列挙の合流 (`pdf_loader.rs:4337` の `(path, password)`)・
  ワーカーの文書キャッシュは、既存のまま実ファイルのパスで動くので世代ごとに自然に分かれる。
- 解決のたびに DB を引かないよう、プロセス内の小さなメモ (論理パス + 元の mtime/size → 世代) を持つ。
  正しさはファイルの不変性に依存し、メモは速度のためだけ。変換完了・削除予約の時点で該当エントリを消す。
- 解決は呼び出し元のスレッド (UI 外) で行うので、ディスパッチャと Critical レーンを DB 待ちで止めない。
- 代償: 削除予約した世代のディスク容量は次回起動まで解放されない。キャッシュ管理画面に「削除予約」と表示する。

### 3.4 文書スタンプ (I4)

スタンプは**実際に読むファイルの (mtime, size)**。既存の DB 列 (`catalog.rs:692` 等の整数ペア) にそのまま収まり、
スキーマ変更が要らない。世代ごとに別ファイルなので、再変換すればスタンプも変わる
(同一秒・同一サイズの再変換で一致する残余リスクは許容。mtime の精度は実装で確認)。

- PDF はスタンプが従来と同じ値になる (挙動不変)。
- UI スレッドで要求を組み立てる箇所 (タイルのサムネイル要求は一覧で採った元ファイルの値を引き継ぐ、`app.rs:79419`) は、
  EPUB について「スタンプはワーカーで解決」と印を付けて渡し、ワーカーが `resolve_read_target` の値で照合する。

対象 (レビュー 1・2 回目で判明したもの。実装 PR で全数を表にする):

| 派生データ | 現状の鮮度判定 | 位置 |
| --- | --- | --- |
| 親フォルダのページ数キャッシュ (`pdf_meta`) | 読み出し = 論理 stat、書き込み = 読んだファイルの stat で不一致 | `app.rs:26695-26743,26908` |
| 一覧タイルのサムネイル要求 | 一覧で採った元ファイルの値 | `app.rs:79419`、照合 `thumb_loader.rs:1185` |
| サムネイルカタログ (本ごとの DB、`page_NNNN` 行) | 行の mtime/size | `catalog.rs:21-33,692` |
| 代表サムネのピンの source ID | `PdfPage` の元パスを stat | `folder_thumb_pins.rs:586,596` |
| 編集プレビューのコンテナ検証 | コンテナの stat | `thumb_loader.rs:1114-1120` |
| 詳細表示のページ数 (DB 読み書き) | 元ファイルの stat | `app/metadata_ops.rs:1417,1456` |
| 外部ツール用ページ PNG | 元ファイルの時刻で再利用 | `materializer.rs:554,571` |
| リモートのページ数 (メモリ・DB) | 論理パスの mtime/size | `remote_ipc/container.rs:6126,6144` |
| 類似画像索引の早期再利用 | 元ファイルの mtime/size | `similar_index.rs:6052` (EPUB 投入可否 `:5752`) |
| 保持ラスタ / final AI | ページ識別のみ | `app.rs:65221-65261` |
| PDF ワーカーの文書キャッシュ | `(path, password, mtime, size)` | 実ファイルのパスで動くので自動的に満たす |

### 3.5 この方式で満たされること

- D3 のキーは `x.epub::page_N` (`edit_source::page_key_for_pdf`、`edit_source.rs:394-396`)。本単位のキー
  (`current_folder` 由来の見開き・読書位置・代表サムネ、`app.rs:21278-21294,44021,21351`) も `x.epub`。
- 本の種類はタイルも中身も PDF。`GridItem` で分岐する箇所 (レーティング種別 `app.rs:56296`、履歴 `:44181,78810`、
  しおり `BookContainerKind::Pdf` `:35346`、代表サムネの許可規則 `:79613`、内容同定 `content_identity.rs:139-154`) は
  PDF の本と揃う。**拡張子で再分類する箇所**は §4.1 の一覧で `is_paged_document_path` へ寄せる。
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

## 4. 構成要素

### 4.1 一覧・分類

- 一覧の分類 (`src/app/folder_scan.rs:547-602`) で `.epub` を `GridItem::PdfFile(path)` にする。
- 変換処理の入力種別は**別の列挙**にする: `ConvertSource::{Archive(ArchiveFormat), Epub}`。`ArchiveFormat` には
  足さない (`scan_summary`・展開・入れ子展開の網羅 match `archive_converter.rs:316,782,977` に「呼ばれてはいけない枝」を
  作らないため)。`from_extension` / `CONVERTIBLE` も変えないので、`ConvertibleArchive` にもリモートのアーカイブ変換ジョブ
  (`remote_ipc/archive_job.rs:1702`) にも入らない。キャッシュ DB の `format` 列の読み書き (`archive_cache.rs:263,347,577-597`) は
  `ConvertSource` を表せる保存種別へ型を分ける (列は TEXT のまま、値 `"epub"` を追加)。
- タイルのサムネイル: 既存の `PdfFile` と同じ要求 (`pdf_page: Some(0)`、`app.rs:79426-79433`)。未変換なら `NotConverted` で
  アイコン + 「EPUB」バッジ。**`NotConverted` を失敗としてログに積まない** (未変換は正常状態)。
- 同名スキップ (D5): `skip_epub_if_pdf_exists`。stem が同じ `.epub` と `.pdf` の `PdfFile` で EPUB を隠す (§4.6)。
- 拡張子で「PDF の本か」を判定している箇所は新設 `folder_tree::is_paged_document_path` (pdf / epub) へ寄せ、
  「実ファイルが PDF か」の問い (パスワード関連) は `is_pdf_extension` のまま残す。レビューで判明した再分類箇所:
  - コレクション: `collection_store/prepare.rs:680,692` (現状 `.epub` は `Unsupported`)
  - タグ一覧: `tag_view.rs:472,488` (現状 `Folder` 側へ落ちる)
  - Remote Web: `crates/remote-web/src/store.rs:306,312,676` (実ファイルの拡張子で再分類し `Pdf` との一致を要求)
  - 詳細欄の種別表示: `ui_main.rs:4610,4650` (現状 `EPUB PDF` と表示)
  - PDF パスワード: `app.rs:26379` (別 PDF のセッション値へのフォールバック) と `:26982` (エラー判定)。EPUB には適用せず、
    パスワードエラーと `NotConverted` を型で分ける
  - その他、非テストの拡張子判定 約 47 か所は実装 PR の分類表で振り分ける (件数ではなく項目で突き合わせる)

### 4.2 開く流れ (未変換の検出は PDF を開く処理の 1 か所)

入口ごとに分岐を足すのではなく、**PDF の本を開く処理 (`load_pdf_as_folder` → 列挙の完了 `poll_pdf_enumerate`) の中で
`NotConverted` を受けて変換要求へ送る**。フォルダ移動 (`app.rs:21772`)、事前走査付き移動 (`:77946`)、履歴・しおり・
コレクション・起動復元のどれから来ても同じ所を通る。

1. `load_pdf_as_folder(x.epub)` が列挙を開始する (スレッド側で解決。§3.3)。
2. 列挙が `NotConverted` を返したら、現状の「その他の失敗 = 空の本として開く」(`app.rs:27026-27035`) には落とさず、
   開く要求の所有 (open-request owner) を引き継いだまま EPUB 変換要求へ移る:
   - `archive_file_handling` が Ignore → 既存のトースト (文言を一般化) で中止。
   - Ask → 確認ダイアログ (要約は変換器の `inspect` をワーカーで実行: レイアウト・綴じ方向・spine 数・DRM 判定)。
   - Convert → 確認なしで変換。
3. 変換ダイアログは既存 `ArchiveConvertState` の相 (`archive_convert.rs:113-188`) を流用し、完了処理は EPUB 専用の分岐にする
   (変換 ZIP 用の「開いた直後の current_folder 同期確認」`:358-393,1197-1241` と `archive_source_override` は通らない)。
   完了後は同じ open-request owner で `load_pdf_as_folder(x.epub)` をやり直す。
4. 変換済みなら列挙がそのまま成功し、通常の PDF の本として開く。

### 4.3 綴じ方向

- 変換器は PDF に `/ViewerPreferences /Direction /R2L` を書く (`crates/epub-pdf-worker/src/render.rs:180`)。
  PDF ワーカーの**列挙応答**に方向を含め (PDFium の viewer preference 読み取り)、非同期列挙の結果とともに UI へ渡す。
  UI スレッドで別途 `get_document_info` を同期に呼ばない (I5)。
- 見開き設定の復元 (`apply_spread_for_key_with_fallback`、`app.rs:21375-21407`) は、方向の正本が `spread_mode`
  (`SpreadMode::reading_direction`、`:21403-21405`) なので、`defaults.reading_direction` を変えるだけでは効かない
  (レビュー 2 回目)。**EPUB の本に保存済みの見開き設定が無いとき**、既定の見開きモードを EPUB の方向に合う版へ写す
  (方向 → モードの対応は既存の `sync_spread_mode_from_reading_direction`、`ui_fullscreen.rs:36473` を正本として使う)。
- 見開き復元は項目の設置時に行う (`app.rs:27952,28135`)。列挙応答は設置より前に届くので、方向は `start_loading_items` の
  引数として渡す。普通の PDF の挙動は変えない (PDF にも適用するかは別途判断)。

### 4.4 変換キャッシュ (`archive_cache`)

- 保存先: `<root>/<hash[..2]>/<hash>/<stem>.g<世代>.pdf` (I6)。世代番号は DB 行を削除しても再利用しない
  (例: 変換時刻のナノ秒、または単調増加カウンタ)。`cache_zip_path_for_data_dir` / `reserve_cache_zip_path`
  (`archive_cache.rs:65-76,542-548`) に出力名を渡せるようにし、ZIP は従来どおり。
- DB: `format` 列に `"epub"`。有効行は常に 1 世代。**削除予約**は別テーブル (パスの一覧) に記録する
  (`archive_cache.db` はリリース済みなので、テーブル追加は `CREATE TABLE IF NOT EXISTS` で行い、既存行は変えない)。
- 起動時、PDF ワーカーを起動する前に削除予約のファイルを消し、消せたものを表から除く。
- 容量上限 (D4): `prune_to_size_limit_locked` (`archive_cache.rs:475-538`) の**合計と候補の両方**から `format = 'epub'` を除く。
- 元ファイル消失の掃除 (`delete_missing_originals`) と手動削除は、EPUB について「有効行の削除 + 削除予約」にする。
- キャッシュ管理画面 (`archive_cache_manager.rs`) の「ZIP」表記を一般化し、形式列に EPUB、状態に「削除予約」を出す。

### 4.5 変換器ワーカー (`crates/epub-pdf-worker`)

本体からは子プロセスとして 1 冊ずつ起動する (プールにしない)。スパイクからの変更点:

- **ネットワーク遮断**: `WebResourceRequested` で仮想ホスト以外への要求をすべて拒否する。
- **本のスクリプトを実行しない**: `IsScriptEnabled = false`。待機判定の `ExecuteScript` がこの状態で動くかを S1 で確認する。
- **待機**: `PeekMessage` + 10ms sleep を `MsgWaitForMultipleObjects` のタイムアウト付き待ちへ替える。
- **進捗**: 標準出力に 1 行 1 JSON。本体は既存の進捗バーへ流す。
- **出力**: 本体が指定した `<世代ファイル>.part` へ書く。本体が検証 (PDFium で開き、ページ数が報告値と一致) してから
  既存の `replace_file_atomic` で公開し、DB の有効行を新世代へ切り替え、旧世代を削除予約へ回す。
- **ユーザーデータフォルダ**: `<temp_root>/epub-<pid>` (materializer の一時領域、`materializer.rs:340-353`)。死んだ PID の掃除 (`:1496-1586`) に含める。
- **子プロセスの後始末 (S1 の合格条件)**: 本体は変換器を `CREATE_SUSPENDED` で起動 → `KILL_ON_JOB_CLOSE` の Job Object へ
  割り当て → 再開する。WebView2 の各 PID が同じ Job に属すること、キャンセル (Job 終了) と本体の強制終了で全プロセスが
  消えることを実測する受入試験を S1 の完了条件にする。成り立たなければ設計を差し戻す (黙った代替策を入れない)。
- **リフローの組版**: 電子書籍端末相当の固定値 (候補 720×1024 CSS px、余白 32px。草枕・moby-dick・ごん狐を画像化して決める)。
  変換プロファイル名 (例 `reflow-v1`) をレポートに出す。
- **終了コード → 本体のエラー**: 2 DRM / 3 不正 / 4 WebView2 Runtime なし / 5 描画失敗 / 6 タイムアウト を型付きエラーへ。

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
  変換して閲覧。文字サイズ変更・テキスト選択は不可。朗読音声・動画は含まれない)。`settings.html` (同名スキップ)。
- 製品ページ `index.html:54,1129`。対外表記は「DRM のない EPUB を変換して閲覧」。
- `privacy.html` / 製品ページ「安心して使えます」: 変換時に WebView2 の一時データを端末内に作ること、通信しないこと。
- 設計文書: `docs/virtual-folders.md` (論理パスと変換世代、読むファイルのスタンプ)、`docs/spec.md`、
  `docs/architecture-overview.md` (新ワーカー)、`docs/async-architecture.md` (子プロセス + Job Object、世代ファイルの寿命)。

## 5. 実装の段階

| 段階 | 内容 | 完了条件 |
| --- | --- | --- |
| S1 | 変換器の仕上げ (§4.5) と Job Object 受入試験 | 27 冊バッチが前回と同等。外部 URL を参照する合成 EPUB で通信ゼロ。WebView2 全 PID の Job 所属と、キャンセル・強制終了後の消滅を実測 |
| S2 | `pdf_loader` 入口での解決 (§3.3)、世代ファイルと削除予約 (§4.4)、スタンプ (§3.4)、開く流れ (§4.2)、綴じ方向 (§4.3) | 状態遷移テスト (未変換→変換→開く→再変換→旧世代が削除予約→再起動で削除→キャッシュ削除→NotConverted→再変換)、I1〜I6 のテスト |
| S3 | 一覧・分類 (§4.1、分類表付き)、同名スキップ (§4.6) | 分類表の全行にテストか理由 |
| S4 | リモート閲覧 (§4.7) | 変換済み EPUB のページ送り・キーが PC と同一 |
| S5 | 配布 (§4.8)、文書 (§4.9) | build-dist が通り、署名・VC runtime 検査に新 exe が含まれる |

## 6. テスト

- 単体: 解決 (pdf / epub 有効世代 / 行なし / mtime 不一致 / 削除予約)、世代番号の非再利用、起動時の削除予約の処理、
  `prune` が EPUB 行を合計にも候補にも含めない、終了コード → エラー型、同名スキップ、`NotConverted` がログに積まれない、
  PDF パスワードのフォールバックが EPUB に効かない。
- 状態遷移: §5 S2 の遷移を 1 本で通す。保存された DB の行を読み、キーが `x.epub::page_N`、本単位のキーが `x.epub`、
  変換世代ファイルのパスがキー・表示・履歴に現れないこと (I1) を確かめる。
- 入口: クリック・フォルダ移動 (Ctrl+↑↓)・事前走査付き移動・履歴・しおり・起動復元のそれぞれで、未変換 EPUB が
  「空の本」にならず変換確認に進むこと。
- I4: 同じ EPUB をページ数の違う世代へ再変換したとき、§3.4 の表の各派生データが古いまま残らないこと。
- 並行: 描画要求の待機中に再変換が完了しても、待機中の要求は旧世代で完了し、以後の要求は新世代を読むこと。
- 綴じ方向: 保存済み見開き設定の無い右開き EPUB が右開きモードで開き、保存済みの本は保存値が勝つこと。
- 実機 (利用者): 漫画 EPUB を開く→右開き→補正・レーティング→キャッシュ削除→再起動→開き直して編集が残る、
  DRM 付き相当でのエラー表示、変換中キャンセルで `msedgewebview2.exe` が残らないこと。

## 7. レビュー 3 回目で確かめてほしい点

1. I6 (実行中は世代ファイルを消さない) によって、第 2 版の R2 系の指摘 (判定順序・原子性・ワーカーの旧文書保持) が
   本当に消えるか。残る競合は無いか (特に変換完了の切り替えと、同時に走っている解決・メモの更新)。
2. `pdf_loader` の公開関数の入口で解決する位置に、解決を迂回して要求を作る経路が残っていないか
   (`LoadRequest` 経由のサムネイル、`HarvestOnCancel`、プリフェッチ、`completed_enumerate_handle` 等)。
3. §4.2 の「`NotConverted` を列挙完了で受けて変換要求へ移る」は、open-request owner と遷移 (fullscreen 予約、
   fs_nav、履歴の巻き戻し) の既存の所有規約と整合するか。
4. §4.3 で方向を列挙応答に含めて `start_loading_items` へ渡す順序で、見開き復元に間に合うか。
5. §3.4 のスタンプ表に残る漏れ。

## 8. レビュー記録

### 第 1 回 (2026-09-25、GPT-6 Sol / xhigh、読み取り専用)

判定: 核 (識別は EPUB、PDF 実体は読み取り境界で解決) は妥当。第 1 版のままの実装には反対。

| # | 指摘 | 対応 |
| --- | --- | --- |
| R1 [P1] | タイル `ConvertibleArchive` と中身 `PdfPage` で本の種類が分裂 | タイルも `PdfFile(x.epub)` (第 2 版)。拡張子で再分類する箇所を追加 (第 3 版 §4.1) |
| R2 [P1] | 固定名キャッシュの上書き・削除と PDF 読者の所有関係が未設計。encode は UI スレッドでも走る | 第 2 版のリース案は不成立 (第 2 回 S1, S2)。第 3 版で I6 と入口での解決へ (§3.3) |
| R3 [P1] | I4 の対象漏れ、`pdf_meta` の読み書き不一致 | 読むファイルのスタンプへ統一、対象表を拡大 (§3.4) |
| R4 [P1] | 右開きを表示へ渡す経路が無い | 列挙応答で方向を渡し、既定の見開きモードへ写す (第 3 版 §4.3) |
| R5 [P2] | リモートのタイル種別・分岐順・アーカイブジョブ除外 | `PdfFile` 化と別列挙で除外。Remote Web の再分類を追加 (§4.7) |
| R6 [P2] | 容量上限から除くなら合計からも除く | 合計と候補の両方から除く (§4.4) |
| R7 [P2] | Job Object の前提が未検証 | S1 の受入試験を完了条件に (§4.5) |
| R8 [P3] | 「PDF バイト列はすべて encode 経由」は誤り | 記述を修正 (§3.1) |

### 第 2 回 (2026-09-25、同じ担当・同じ会話の継続)

判定: もう一度設計を詰める必要がある。核は維持できる。

| # | 指摘 | 対応 (第 3 版) |
| --- | --- | --- |
| S1 [P1] | ディスパッチャで解決する位置が、取り出し前の open admission と矛盾。待機中要求が旧世代で完了する条件とも矛盾 | 解決を `pdf_loader` 入口 (要求の組み立て前) へ移し、要求全体を実ファイルで動かす (§3.3) |
| S2 [P1] | DB 行の切替とリース登録が非原子的 | リースを廃止。実行中は世代ファイルを消さない (I6) |
| S3 [P1] | `defaults.reading_direction` は方向付きの `spread_mode` に上書きされる。UI での同期取得は I5 違反 | 既定の見開きモードへ写す。方向は列挙応答で非同期に渡す (§4.3) |
| S4 [P1] | コレクション・タグ・Remote Web が拡張子で再分類し、`.epub` を PDF 本として扱わない | 再分類箇所を §4.1 に列挙し `is_paged_document_path` へ寄せる |
| S5 [P2] | スタンプの伝達先と永続表現が未定義 (代表ピン・編集プレビュー・詳細ページ数も漏れ) | 読むファイルの (mtime, size) を既存の整数ペアへ。対象表に追加 (§3.4) |
| S6 [P2] | フォルダ移動等が変換導線を通らず、未変換 EPUB が「空の PDF 本」になる | `NotConverted` を列挙完了の 1 か所で受けて変換要求へ (§4.2) |
| S7 [P2] | PDFium ワーカーが旧文書を保持し、個別 close が無い。削除の完了状態が曖昧 | 削除予約テーブル + 起動時 (ワーカー起動前) に削除。管理画面に「削除予約」表示 (§4.4) |
| S8 [P3] | 詳細欄 `EPUB PDF` 表示、PDF パスワードのフォールバック | §4.1 の再分類一覧に追加。パスワードエラーと `NotConverted` を型で分ける |
| Q6 | `ArchiveFormat::Epub` か別列挙か | 別列挙 `ConvertSource` を採用 (§4.1) |
