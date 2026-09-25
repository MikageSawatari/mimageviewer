# EPUB → PDF 変換の本体組み込み設計 (2026-09-25、第 2 版 = 独立レビュー 1 回目の反映)

- 経緯と検証: `docs/backlog-on-hold.md` §1.247 (計画)、`docs/epub-pdf-spike-results.md` (スパイク結果)。
- 変換器: `crates/epub-pdf-worker` (単体 CLI `mimageviewer-epub-pdf`)。
- 状態: **設計案 第 2 版。独立レビュー 2 回目待ち**。第 1 版への指摘と対応は §8。
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
2. 開くと、未変換なら確認ダイアログ → 変換 (進捗・キャンセル可) → 開く。変換済みなら即座に開く。確認の要否は既存の `archive_file_handling` (Ask / Convert / Ignore) に従う。
3. 開いた後のアドレスバー・親へ戻る・次回起動の復元・履歴・しおりは、すべて `book.epub` を指す。PDF の本と同じ種類として扱われる。
4. DRM 付き / 壊れた EPUB / WebView2 Runtime なし / タイムアウトは、それぞれ理由を明示したエラー。空の本にはしない。
5. 本に保存された綴じ方向が無ければ、EPUB の `page-progression-direction` に従う (右開きの本は右開きで開く)。

## 3. 中心設計

### 3.1 考え方: 論理パスは EPUB、PDFium が読むのは変換世代のファイル

EPUB を「**中身のバイト列が変換キャッシュに置いてある PDF の本**」として扱う。

- アプリ内で本とページを識別するパス (論理パス) は、**どこでも `x.epub`**。一覧のタイルは `GridItem::PdfFile(x.epub)`、
  中身は `GridItem::PdfPage { pdf_path: x.epub, .. }`、`current_folder = x.epub`。PDF の本と同じ種類。
- PDFium にファイルを開かせる処理は、すべて PDF ワーカープールへの要求 (`pdf_loader` の `encode_*_request` →
  `pool.execute`、`src/pdf_loader.rs:884-964,3919-4640`) を通る。**論理パス → 変換世代ファイルの解決は、この投入境界で
  1 回だけ行う** (§3.3)。
- PDFium 以外で本のファイル自体を扱う処理 (内容同定のハッシュ `content_identity.rs:1786`、タイルの「外部で開く」
  `external_tool.rs:223`、ドラッグ出し `ui_main.rs:1917`、コピー・削除・改名) は、**論理パス = 元 EPUB を扱うのが正しい**
  (本のファイルは EPUB であり、変換 PDF はキャッシュにすぎない)。これらは変更しない。PDF ページの外部渡しは
  PNG 化 (`materializer.rs:244-249`) なので、描画は投入境界を通る。

### 3.2 不変条件

- **I1 識別の一意性**: 本・ページの識別・永続キー・表示・履歴に使うパスは `x.epub`。変換世代ファイルのパスは
  投入境界の外へ出さない (ログを除く)。
- **I2 解決は投入境界で 1 回**: `x.epub` → 変換世代ファイルの解決は PDF プールへの投入境界だけが行い、その結果
  (読み取りリース) を要求の完了まで保持する。呼び出し側は解決を知らない。
- **I3 未変換は型付きエラー**: 有効な変換世代が無い / 元 EPUB の mtime・size が記録と違う場合、解決は
  `NotConverted` を返す。空の本・黙った fallback にしない。上位はこれを変換導線へ送るか、アイコン表示にする。
- **I4 派生データは文書スタンプで検証**: PDF の中身から作った派生データの鮮度判定には、論理パスの stat ではなく
  **文書スタンプ** (§3.4) を使う。
- **I5 UI スレッドで DB もファイルも引かない**: 解決と文書スタンプ取得は UI スレッドで同期に DB・ファイルへ触れない。
- **I6 変換世代ファイルは不変**: 一度公開した変換世代ファイルは書き換えない。再変換は新しい世代を作る。
  古い世代は読み取りリースが 0 になってから消す。

### 3.3 解決の場所と読み取りリース

第 1 版は `encode_*_request` 内とプロセス共通の可変パス表で解決する案だったが、次の理由で退けた (§8 R2):
`enumerate_pages_async` は UI スレッドで `encode_enumerate_request` を実行する (`app.rs:26491`、`pdf_loader.rs:4661`)。
PDF プールは投入済み要求のパスを保持して後で開く (`pdf_loader.rs:2555,3322`)。固定名のキャッシュを上書き・削除すると
待機中の要求や開いたままの別ウィンドウと整合しない。

第 2 版:

- PDF プールへの要求は**論理パス**を持って投入する。プールのディスパッチャ (ワーカー側スレッド、UI スレッドではない)
  が、ジョブを取り出して PDFium ワーカーへ送る直前に解決する:
  `resolve(logical) -> Result<ReadLease, PdfReadError>`。
  - 拡張子 pdf → `ReadLease { read_path: logical, stamp: FileStamp(mtime, size) }` (リース管理不要)。
  - 拡張子 epub → `archive_cache.db` の有効行 (元の mtime・size 一致) から現在の世代を得て
    `ReadLease { read_path: <世代ファイル>, stamp: Generation(id) }`。行が無い・不一致 → `NotConverted`。
- `ReadLease` はジョブ完了まで保持され、世代ごとの参照カウントを持つ (プロセス内)。
- `enumerate_pages_async` の要求組み立て (encode) は、生成するスレッド側へ移す (UI スレッドで encode しない)。
  この変更は PDF 全体に効くが、要求の中身は変わらない。
- 世代の廃止 (再変換・キャッシュ削除・元ファイル消失): DB の行を先に更新して新規の解決を止める → 参照カウント 0 で
  ファイルを削除。PDFium ワーカーの文書キャッシュが古い世代を開いたまま持っている場合は削除に失敗し得るので、
  失敗した世代ファイルは「廃止済み」として記録し、起動時の掃除で消す (Windows のファイルロックに依存した
  即時削除を前提にしない)。

### 3.4 文書スタンプ (I4)

`pdf_document_stamp(logical) -> DocumentStamp` を 1 つ用意し、派生データの鮮度判定はすべてこれに寄せる。

- PDF: `FileStamp(mtime, size)` (現状と同じ意味)。
- EPUB: `Generation(id)` (世代 ID は変換ごとに増える。変換世代ファイルの名前にも含める)。
- UI スレッドから呼ばれる箇所では、開いている本について**既に解決済みのスタンプ**を渡す (I5)。

対象 (レビューで判明したものを含む。実装 PR でさらに全数を表にする):

| 派生データ | 現状の鮮度判定 | 位置 |
| --- | --- | --- |
| 親フォルダのページ数キャッシュ (`pdf_meta`) | 読み出し = 論理パスの stat、書き込み = 読んだファイルの stat で**不一致** | `app.rs:26695-26743,26908` |
| 外部ツール用ページ PNG | 元ファイルの時刻で再利用 | `materializer.rs:554,571` |
| リモートのページ数 (メモリ・DB) | 論理パスの mtime/size | `remote_ipc/container.rs:6126,6144` |
| 類似画像索引の早期再利用 | 元ファイルの mtime/size | `similar_index.rs:6052` (EPUB 投入可否は `:5752`) |
| 保持ラスタ / final AI | ページ識別のみ | `app.rs:65221-65261` |
| サムネイルカタログ (本ごとの DB) | 識別パスのハッシュで DB を分ける (鮮度保証ではない) | `catalog.rs:21-33` |
| PDF ワーカーの文書キャッシュ | `(path, password, mtime, size)` | `virtual-folders.md:367-371` (解決後のパスで自動的に満たす) |
| ページの content type | 描画時に確定 | `app.rs:73352` |

### 3.5 この方式で満たされること

- D3 のキーは `x.epub::page_N` (`edit_source::page_key_for_pdf`、`edit_source.rs:394-396`)。本単位のキー
  (`current_folder` 由来の見開き・読書位置・代表サムネ、`app.rs:21278-21294,44021,21351`) も `x.epub`。
- 本の種類はタイルも中身も PDF で一貫する。レーティングの種別 (`app.rs:56296`)、タグ一覧の再分類 (`tag_view.rs:472`)、
  履歴 (`app.rs:44181,78810`)、しおりの `BookContainerKind::Pdf` (`app.rs:35346`)、代表サムネの許可規則
  (`app.rs:79613`)、内容同定の種別 (`content_identity.rs:139-154`) が PDF の本と同じ扱いになる。
- mIV 内での移動・改名は既存の汎用キー移行が追従する (`.pdf` 限定なのはパスワード移行のみ、
  `rename_key_migration.rs:1314,1334,2022`)。キャッシュ行は元パスのハッシュで決まるので、改名後は再変換になり、
  旧行は「元ファイル消失」の掃除で回収される。
- `archive_source_override` を使わない。変換 ZIP 用の「開いた直後の同期確認」(`archive_convert.rs:358,1205`) を
  通らない EPUB 専用の完了分岐にする。EPUB を開くとき `start_loading_items` が残存 override を消すこと
  (`app.rs:27737-27742,27923-27933`) をテストで固定する。
- 別ウィンドウの記述子 `ViewerContextDescriptor::Pdf` (`app.rs:2633`) は変更不要。detached 凍結対象の経路に分岐を足さない。

### 3.6 却下した案

| 案 | 内容 | 却下理由 |
| --- | --- | --- |
| A | `PdfPage` に識別用パスの欄を足す | 意味的変更 60〜90 か所。保存済みキーから組み立て直す経路が描けない項目を作る危険が高い |
| B | `archive_source_override` を PDF にも使う | ページのキーが変換 PDF のパスのまま (D3 を満たさない)。override は変換 ZIP 前提の分岐が多い |
| C | 変換 7z と同じく、キーは変換 PDF のパス | 元 EPUB の移動・改名でキーが孤立。サイドカーが APPDATA に書かれる。ドライブ文字違いが同じキャッシュを共有 |
| 第 1 版 | タイルを `ConvertibleArchive`、中身を `PdfPage`。解決は `encode_*_request` と可変パス表 | 本の種類が分裂 (§8 R1)、UI スレッドでの解決・固定名上書きと読者の競合 (§8 R2) |

## 4. 構成要素

### 4.1 一覧での扱い

- 一覧の分類 (`src/app/folder_scan.rs:547-602`) で `.epub` を `GridItem::PdfFile(path)` にする。
  `ArchiveFormat::from_extension` / `CONVERTIBLE` には**入れない** (`ConvertibleArchive` にも、リモートの
  アーカイブ変換ジョブ `remote_ipc/archive_job.rs:1702` にも入らない)。
- 変換処理の内部 (変換ダイアログ・キャッシュ DB の `format` 列) で EPUB を区別する必要があるので、変換器側の
  入力種別として別の列挙 (仮称 `ConvertSource::{Archive(ArchiveFormat), Epub}`) を持つか、`ArchiveFormat::Epub` を
  足して `from_extension` からは返さない。どちらにするかは実装時に網羅 match の数で決める (レビュー観点)。
- タイルのサムネイル: 既存の `PdfFile` と同じ要求 (`pdf_page: Some(0)`、`app.rs:79426-79433`) を論理パスで出す。
  未変換なら `NotConverted` が返り、アイコン + 「EPUB」バッジ。**`NotConverted` を失敗としてログに積まない**
  (未変換は正常状態)。
- 同名スキップ (D5): `skip_epub_if_pdf_exists`。`PdfFile` 同士 (stem 同一、拡張子 epub と pdf) で EPUB を隠す。
- 「PDF 系の本か」という問いは新設 `folder_tree::is_paged_document_path` (pdf / epub) へ寄せ、
  「実ファイルが PDF か」(パスワード保存 `rename_key_migration.rs:2022` 等) は `is_pdf_extension` のまま残す。
  非テストの拡張子判定 約 47 か所それぞれを実装 PR の分類表で振り分ける (件数ではなく項目で突き合わせる)。

### 4.2 開く流れ

`load_folder_or_convert_archive_with_auto_fullscreen_owned` (`src/app.rs:21807-21917`) に EPUB 分岐を足す:

1. `archive_file_handling` が Ignore → 既存のトースト (文言を一般化)。
2. 変換キャッシュに有効な世代がある (ワーカースレッドで判定) → `load_pdf_as_folder(x.epub)`。
3. 無い → EPUB 用の変換ダイアログ (既存 `ArchiveConvertState` の相を流用、`archive_convert.rs:113-188`) →
   完了 → `load_pdf_as_folder(x.epub)`。変換 ZIP 用の完了処理 (`archive_convert.rs:358-393,1197-1241`) は通らない。
4. 保存済み参照 (履歴・しおり・コレクション・起動復元・リモート) から開いて `NotConverted` が返った場合も、
   同じ変換導線へ送る。
5. 確認ダイアログの要約は変換器の `inspect` (WebView2 を使わない解析) をワーカースレッドで実行し、
   「レイアウト (固定 / リフロー)・綴じ方向・spine 数・DRM 判定」を出す。
6. 綴じ方向 (§2-5): `get_document_info` が PDF の `/ViewerPreferences /Direction` を返すよう拡張し、
   **EPUB の本に限り**、見開き設定の復元 (`apply_spread_for_key_with_fallback`、`app.rs:21375-21395`) の既定値
   `defaults.reading_direction` に使う。本ごとの保存値があればそちらが勝つ。普通の PDF の挙動は変えない
   (PDF にも適用するかは別途判断)。

### 4.3 変換キャッシュ (`archive_cache`)

- 保存先: `<root>/<hash[..2]>/<hash>/<stem>.g<世代>.pdf` (I6)。`cache_zip_path_for_data_dir` /
  `reserve_cache_zip_path` (`archive_cache.rs:65-76,542-548`) に出力名を渡せるようにし、ZIP は従来どおり。
- DB: `format` 列に `"epub"`。世代は保存先パス (`cached_zip_path` 列) に含まれるので、列の追加は不要
  (列名が ZIP を指すのは既存の命名。移行不要)。
- 容量上限 (D4): `prune_to_size_limit_locked` (`archive_cache.rs:475-538`) の**合計と候補の両方**から
  `format = 'epub'` を除く。`total_size` の表示は内訳 (ZIP / EPUB) を出す。
- 元ファイル消失の掃除 (`delete_missing_originals`) と手動削除は EPUB も対象。削除は §3.3 の世代廃止手順を通る。
- キャッシュ管理画面 (`src/ui_dialogs/archive_cache_manager.rs`) の「ZIP」表記を一般化し、形式列に EPUB を出す。

### 4.4 変換器ワーカー (`crates/epub-pdf-worker`)

本体からは子プロセスとして 1 冊ずつ起動する (プールにしない)。スパイクからの変更点:

- **ネットワーク遮断**: `WebResourceRequested` で仮想ホスト以外への要求をすべて拒否する。
- **本のスクリプトを実行しない**: `IsScriptEnabled = false`。待機判定の `ExecuteScript` がこの状態で動くかを S1 で確認する。
- **待機**: `PeekMessage` + 10ms sleep を `MsgWaitForMultipleObjects` のタイムアウト付き待ちへ替える。
- **進捗**: 標準出力に 1 行 1 JSON。本体は既存の進捗バーへ流す。
- **出力**: 本体が指定した `<世代ファイル>.part` へ書く。本体が検証 (PDFium で開き、ページ数が報告値と一致) してから
  既存の `replace_file_atomic` で公開する (`archive_converter.rs:726-742`)。
- **ユーザーデータフォルダ**: `<temp_root>/epub-<pid>` (materializer の一時領域、`materializer.rs:340-353`)。
  死んだ PID の掃除 (`:1496-1586`) に含める。
- **子プロセスの後始末 (S1 の合格条件)**: 本体は変換器を `CREATE_SUSPENDED` で起動 → `KILL_ON_JOB_CLOSE` の Job Object へ
  割り当て → 再開する。WebView2 のブラウザ・レンダラ等の各 PID が同じ Job に属すること、キャンセル (Job 終了) と本体の
  強制終了で全プロセスが消えることを**実測する受入試験**を S1 の完了条件にする。成り立たなければ設計を差し戻す
  (黙った代替策を入れない)。
- **リフローの組版**: 電子書籍端末相当の固定値 (候補 720×1024 CSS px、余白 32px。草枕・moby-dick・ごん狐を画像化して決める)。
  変換プロファイル名 (例 `reflow-v1`) をレポートに出す。
- **終了コード → 本体のエラー**: 2 DRM / 3 不正 / 4 WebView2 Runtime なし / 5 描画失敗 / 6 タイムアウト を型付きエラーへ。

### 4.5 配布

mIV Remote のサービス exe と同じ形 (cargo でビルド、launcher が内包、ポータブルは exe の隣、本体は自分の exe の隣から探す。
`src/remote_ipc/service.rs:41-46` と同型 + `MIV_EPUB_PDF_WORKER` での上書き)。

触る箇所: `crates/launcher/build.rs:31-64,112-117`、`crates/launcher/src/main.rs:23,87-91` (+ テスト `:584-610`)、
`scripts/build-release.ps1` / `.sh`、`scripts/build-dev.ps1`、`scripts/build-portable.ps1`、`scripts/build-dist.ps1`
(ビルド・存在確認・複製・署名一覧・プロセス停止一覧・clean・最終 PE 一覧)、`installer/readme*.txt`、
`installer/mimageviewer.iss` のコメント、`CLAUDE.md` Distribution 節、`docs/portable-build-plan.md`。
x64 で `+crt-static` なので VC runtime 検査の追加設定は不要 (`check-vcrt-pe-dependencies.ps1:67-73`)。

### 4.6 同名スキップ設定 (D5)

`skip_epub_if_pdf_exists` (既定 true)。`skip_archive_if_zip_exists` と同じ箇所に並べる:
`settings.rs:4515` 付近と `Default`、`folder_scan.rs:258-264,790-853`、`folder_tree.rs:25-48,701-738`、
`smart_folder.rs:3503-3769`、`settings_db.rs` のリモート一覧設定 5 か所 (`:54-61,298,354,375,2923`)、
環境設定 `preferences/pages.rs:8603-8609`・検索索引 `search_index.rs:534-539`・再読込タプル `preferences.rs:2291-2299,2591-2599`。

### 4.7 リモート (mIV Remote)

- 初版: **変換済みの EPUB を閲覧できる**ところまで。タイルは `PdfFile` なのでリモートにも PDF 系の本として送られる。
  リモートの PDF 判定 (論理パスの拡張子、`remote_ipc/container.rs:3139,3300,4384,4460,5643,5671,7041-7069` 付近) を
  `is_paged_document_path` へ置き換え、アーカイブ分岐が先に来る箇所 (`:3455,4458,5633`) で EPUB がアーカイブ扱いに
  ならないことを確認する。読み取りは §3.3 の投入境界を通る。
- EPUB はアーカイブ変換ジョブの対象外 (§4.1)。未変換の EPUB を開くと「PC で一度開いて変換してください」。

### 4.8 文書

- マニュアル: `formats.html:167-179` と `faq.html:90,168-176` の「EPUB 非対応」を書き換え (DRM のない EPUB を固定レイアウトへ
  変換して閲覧。文字サイズ変更・テキスト選択は不可。朗読音声・動画は含まれない)。`settings.html` (同名スキップ)。
- 製品ページ `index.html:54,1129`。対外表記は「DRM のない EPUB を変換して閲覧」。
- `privacy.html` / 製品ページ「安心して使えます」: 変換時に WebView2 の一時データを端末内に作ること、通信しないこと。
- 設計文書: `docs/virtual-folders.md` (論理パスと変換世代、文書スタンプ)、`docs/spec.md`、`docs/architecture-overview.md`
  (新ワーカー)、`docs/async-architecture.md` (子プロセス + Job Object、読み取りリース)。

## 5. 実装の段階

| 段階 | 内容 | 完了条件 |
| --- | --- | --- |
| S1 | 変換器の仕上げ (§4.4) と Job Object 受入試験 | 27 冊バッチが前回と同等。外部 URL を参照する合成 EPUB で通信ゼロ。WebView2 全 PID の Job 所属と、キャンセル・強制終了後の消滅を実測 |
| S2 | 投入境界での解決・読み取りリース・世代ファイル (§3.3)、文書スタンプ (§3.4)、キャッシュ (§4.3)、開く流れ (§4.2) | 状態遷移テスト (未変換→変換→開く→再変換→旧世代の廃止→キャッシュ削除→NotConverted→再変換)、I1〜I6 のテスト |
| S3 | 一覧 (§4.1)、判定の一本化 (分類表付き)、同名スキップ (§4.6)、綴じ方向 | 分類表の全行にテストか理由 |
| S4 | リモート閲覧 (§4.7) | 変換済み EPUB のページ送り・キーが PC と同一 |
| S5 | 配布 (§4.5)、文書 (§4.8) | build-dist が通り、署名・VC runtime 検査に新 exe が含まれる |

## 6. テスト

- 単体: 解決 (pdf / epub 有効世代 / 行なし / mtime 不一致)、読み取りリースの参照カウントと世代廃止、`prune` が
  EPUB 行を合計にも候補にも含めない、終了コード → エラー型、同名スキップ、`NotConverted` がログに積まれない。
- 状態遷移: §5 S2 の遷移を 1 本で通す。保存された DB の行を読み、キーが `x.epub::page_N`、本単位のキーが `x.epub`、
  変換世代ファイルのパスがキー・表示・履歴に現れないこと (I1) を確かめる。
- I4: 同じ EPUB をページ数の違う世代へ再変換したとき、§3.4 の表の各派生データが古いまま残らないこと。
- 並行: 描画要求が待機中に再変換・キャッシュ削除が起きても、待機中の要求は旧世代で完了し、新規要求は新世代 /
  `NotConverted` になること。
- 実機 (利用者): 漫画 EPUB を開く→右開き→補正・レーティング→キャッシュ削除→開き直して編集が残る、DRM 付き相当でのエラー表示、
  変換中キャンセルで `msedgewebview2.exe` が残らないこと。

## 7. レビュー 2 回目で確かめてほしい点

1. §3.3 の投入境界 (ディスパッチャが要求を PDFium ワーカーへ送る直前) で解決する案は、PDF プールの実際の構造
   (`JobQueue` / `run_dispatcher`、優先度・epoch・harvest) に無理なく入るか。解決に DB を引く時間がディスパッチャを
   止めないか (Critical 要求の遅延)。
2. `enumerate_pages_async` の encode をスレッド側へ移すことに副作用は無いか。他に UI スレッドで encode している経路は無いか。
3. 世代廃止時、PDFium ワーカーが古い世代を開いたまま保持している場合の扱い (起動時掃除へ回す) で足りるか。
   ワーカーへ「この文書を閉じよ」を送る仕組みが既にあるか。
4. `.epub` を `PdfFile` にしたとき、`PdfFile` を前提に実ファイルを PDF として扱う箇所 (パスワード入力、PDF 情報表示、
   印刷・外部 PDF ビューアで開く等) に EPUB が入って誤動作しないか。
5. 文書スタンプの対象 (§3.4 の表) に残る漏れ。
6. §4.1 の「変換器の入力種別」を `ArchiveFormat::Epub` (from_extension からは返さない) にするか別列挙にするか。
   既存の網羅 match と `format` 列の扱いから、どちらが安全か。

## 8. レビュー記録

### 第 1 回 (2026-09-25、GPT-6 Sol / xhigh、読み取り専用)

判定: 核 (識別は EPUB、PDF 実体は読み取り境界で解決) は妥当。第 1 版のままの実装には反対。

| # | 指摘 | 対応 (第 2 版) |
| --- | --- | --- |
| R1 [P1] | タイル `ConvertibleArchive` と中身 `PdfPage` で本の種類が分裂 (レーティング種別・代表サムネ・履歴・しおり・コレクション事前検査・内容同定) | タイルも `PdfFile(x.epub)` に統一 (§3.5, §4.1) |
| R2 [P1] | 固定名キャッシュの上書き・削除と PDF 読者 (待機中要求・別ウィンドウ) の所有関係が未設計。`encode_*_request` は UI スレッドでも走る | 投入境界で解決・読み取りリース・世代ごとの不変ファイル (§3.3, I6) |
| R3 [P1] | I4 の対象漏れ (materializer PNG、リモートのページ数、類似画像索引、保持ラスタ、代表サムネ)。`pdf_meta` は読み書きでキーが不一致 | 文書スタンプを 1 つ用意して寄せる (§3.4) |
| R4 [P1] | 右開きを表示へ渡す経路が無い | `get_document_info` で方向を返し、EPUB の既定値に使う (§4.2-6) |
| R5 [P2] | リモートはタイル種別・分岐順・アーカイブジョブの除外が必要 | `PdfFile` 化と `from_extension` 除外で解消。分岐順は確認項目 (§4.7) |
| R6 [P2] | 容量上限から除くなら合計からも除く | 合計と候補の両方から除く (§4.3) |
| R7 [P2] | Job Object で WebView2 全プロセスが終わる前提は未検証 | S1 の受入試験を完了条件に (§4.4) |
| R8 [P3] | 「PDF バイト列はすべて encode 経由」は誤り (内容同定のハッシュ、外部で開く) | 記述を修正。これらは元 EPUB を扱うのが正しいと明記 (§3.1) |
